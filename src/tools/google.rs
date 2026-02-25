//! Google Workspace tool for Gmail and Calendar operations.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use gog_calendar::create::{create_event, CreateParams};
use gog_calendar::freebusy::query_freebusy;
use gog_calendar::list::{list_events, ListParams};
use gog_calendar::types::EventDateTime;
use gog_gmail::get::{get_message, MessageFormat};
use gog_gmail::search::{search_messages, SearchParams};
use gog_gmail::send::{send_message, SendParams};

use crate::error::{Result, ZeptoError};

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Actions that modify external state and require user confirmation.
const DANGEROUS_ACTIONS: &[&str] = &["gmail_send", "gmail_reply", "calendar_create"];

/// Google Workspace tool for Gmail and Google Calendar operations.
///
/// Supports 7 actions:
/// - `gmail_search`: Search Gmail messages by query
/// - `gmail_read`: Read a full Gmail message by ID
/// - `gmail_send`: Send a new email
/// - `gmail_reply`: Reply to an existing email thread
/// - `calendar_list`: List upcoming calendar events
/// - `calendar_create`: Create a new calendar event
/// - `calendar_freebusy`: Query free/busy status for calendars
#[derive(Debug)]
pub struct GoogleTool {
    client: Client,
    access_token: String,
    default_calendar: String,
    max_search_results: u32,
}

impl GoogleTool {
    /// Create a new GoogleTool with the given OAuth access token.
    ///
    /// # Arguments
    /// * `access_token` - OAuth 2.0 access token with Gmail and Calendar scopes
    /// * `default_calendar` - Default calendar ID (use "primary" for the user's primary calendar)
    /// * `max_search_results` - Maximum results to return for Gmail search
    pub fn new(access_token: &str, default_calendar: &str, max_search_results: u32) -> Self {
        Self {
            client: Client::new(),
            access_token: access_token.to_string(),
            default_calendar: default_calendar.to_string(),
            max_search_results,
        }
    }

    /// Return `true` when the given action modifies external state (send/create).
    pub fn is_dangerous_action(action: &str) -> bool {
        DANGEROUS_ACTIONS.contains(&action)
    }
}

#[async_trait]
impl Tool for GoogleTool {
    fn name(&self) -> &str {
        "google"
    }

    fn description(&self) -> &str {
        "Google Workspace tool for Gmail and Calendar operations. Actions: gmail_search, gmail_read, gmail_send, gmail_reply, calendar_list, calendar_create, calendar_freebusy."
    }

    fn compact_description(&self) -> &str {
        "Gmail+Calendar"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Messaging
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "gmail_search",
                        "gmail_read",
                        "gmail_send",
                        "gmail_reply",
                        "calendar_list",
                        "calendar_create",
                        "calendar_freebusy"
                    ],
                    "description": "The Google Workspace operation to perform."
                },
                "query": {
                    "type": "string",
                    "description": "Gmail search query (e.g. 'from:alice subject:hello'). Required for gmail_search."
                },
                "message_id": {
                    "type": "string",
                    "description": "Gmail message ID. Required for gmail_read."
                },
                "to": {
                    "type": "string",
                    "description": "Recipient email address. Required for gmail_send and gmail_reply."
                },
                "subject": {
                    "type": "string",
                    "description": "Email subject line. Required for gmail_send and gmail_reply."
                },
                "body": {
                    "type": "string",
                    "description": "Email body text. Required for gmail_send and gmail_reply."
                },
                "thread_id": {
                    "type": "string",
                    "description": "Thread ID to reply into. Required for gmail_reply."
                },
                "html": {
                    "type": "boolean",
                    "description": "When true, treat body as HTML. Optional for gmail_send/gmail_reply."
                },
                "calendar_id": {
                    "type": "string",
                    "description": "Calendar identifier. Defaults to the configured default calendar. Optional for calendar_list and calendar_create."
                },
                "time_min": {
                    "type": "string",
                    "description": "Lower bound for event time (RFC3339). Optional for calendar_list; required for calendar_freebusy."
                },
                "time_max": {
                    "type": "string",
                    "description": "Upper bound for event time (RFC3339). Optional for calendar_list; required for calendar_freebusy."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of events to return. Optional for calendar_list."
                },
                "summary": {
                    "type": "string",
                    "description": "Event title. Required for calendar_create."
                },
                "start": {
                    "type": "string",
                    "description": "Event start time (RFC3339). Required for calendar_create."
                },
                "end": {
                    "type": "string",
                    "description": "Event end time (RFC3339). Required for calendar_create."
                },
                "description": {
                    "type": "string",
                    "description": "Event description / notes. Optional for calendar_create."
                },
                "location": {
                    "type": "string",
                    "description": "Event location. Optional for calendar_create."
                },
                "attendees": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Attendee email addresses. Optional for calendar_create; optional calendar IDs for calendar_freebusy."
                },
                "calendars": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Calendar IDs to query for freebusy. Optional for calendar_freebusy (defaults to default_calendar)."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'action' parameter".to_string()))?;

        let output = match action {
            "gmail_search" => self.gmail_search(&args).await?,
            "gmail_read" => self.gmail_read(&args).await?,
            "gmail_send" => self.gmail_send(&args, false).await?,
            "gmail_reply" => self.gmail_send(&args, true).await?,
            "calendar_list" => self.calendar_list(&args).await?,
            "calendar_create" => self.calendar_create(&args).await?,
            "calendar_freebusy" => self.calendar_freebusy(&args).await?,
            other => {
                return Err(ZeptoError::Tool(format!("Unknown action '{}'", other)));
            }
        };

        Ok(ToolOutput::llm_only(output))
    }
}

// ---------------------------------------------------------------------------
// Action implementations
// ---------------------------------------------------------------------------

impl GoogleTool {
    async fn gmail_search(&self, args: &Value) -> Result<String> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'query' for gmail_search".to_string()))?;

        let params = SearchParams {
            query: query.to_string(),
            max_results: Some(self.max_search_results),
            ..Default::default()
        };

        let result = search_messages(&self.client, &self.access_token, &params)
            .await
            .map_err(|e| ZeptoError::Tool(format!("Gmail search failed: {}", e)))?;

        if result.messages.is_empty() {
            return Ok("No messages found.".to_string());
        }

        let mut lines = Vec::new();
        lines.push(format!(
            "Found {} message(s) (estimate: {}):",
            result.messages.len(),
            result.result_size_estimate.unwrap_or(0)
        ));
        for msg in &result.messages {
            lines.push(format!("  ID: {}  Thread: {}", msg.id, msg.thread_id));
        }
        if result.next_page_token.is_some() {
            lines.push("(more results available)".to_string());
        }

        Ok(lines.join("\n"))
    }

    async fn gmail_read(&self, args: &Value) -> Result<String> {
        let message_id = args
            .get("message_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'message_id' for gmail_read".to_string()))?;

        let msg = get_message(
            &self.client,
            &self.access_token,
            message_id,
            MessageFormat::Full,
        )
        .await
        .map_err(|e| ZeptoError::Tool(format!("Gmail read failed: {}", e)))?;

        let mut lines = Vec::new();
        lines.push(format!("Message ID: {}", msg.id));
        lines.push(format!("Thread ID: {}", msg.thread_id));

        // Extract headers from payload
        if let Some(payload) = &msg.payload {
            if let Some(headers) = &payload.headers {
                for header in headers {
                    match header.name.as_str() {
                        "Subject" | "From" | "To" | "Date" | "Cc" => {
                            lines.push(format!("{}: {}", header.name, header.value));
                        }
                        _ => {}
                    }
                }
            }
        }

        lines.push(format!("Snippet: {}", msg.snippet));

        Ok(lines.join("\n"))
    }

    async fn gmail_send(&self, args: &Value, is_reply: bool) -> Result<String> {
        let to = args.get("to").and_then(Value::as_str).ok_or_else(|| {
            let action = if is_reply {
                "gmail_reply"
            } else {
                "gmail_send"
            };
            ZeptoError::Tool(format!("Missing 'to' for {}", action))
        })?;

        let subject = args.get("subject").and_then(Value::as_str).ok_or_else(|| {
            let action = if is_reply {
                "gmail_reply"
            } else {
                "gmail_send"
            };
            ZeptoError::Tool(format!("Missing 'subject' for {}", action))
        })?;

        let body = args.get("body").and_then(Value::as_str).ok_or_else(|| {
            let action = if is_reply {
                "gmail_reply"
            } else {
                "gmail_send"
            };
            ZeptoError::Tool(format!("Missing 'body' for {}", action))
        })?;

        let thread_id = if is_reply {
            let tid = args
                .get("thread_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ZeptoError::Tool("Missing 'thread_id' for gmail_reply".to_string())
                })?;
            Some(tid.to_string())
        } else {
            args.get("thread_id")
                .and_then(Value::as_str)
                .map(String::from)
        };

        let params = SendParams {
            to: vec![to.to_string()],
            subject: subject.to_string(),
            body: body.to_string(),
            html: args.get("html").and_then(Value::as_bool).unwrap_or(false),
            thread_id,
            ..Default::default()
        };

        let sent = send_message(&self.client, &self.access_token, "me", &params)
            .await
            .map_err(|e| ZeptoError::Tool(format!("Gmail send failed: {}", e)))?;

        let action_label = if is_reply { "Reply sent" } else { "Email sent" };
        Ok(format!(
            "{} successfully. Message ID: {}  Thread ID: {}",
            action_label, sent.id, sent.thread_id
        ))
    }

    async fn calendar_list(&self, args: &Value) -> Result<String> {
        let calendar_id = args
            .get("calendar_id")
            .and_then(Value::as_str)
            .unwrap_or(&self.default_calendar)
            .to_string();

        let params = ListParams {
            calendar_id,
            time_min: args
                .get("time_min")
                .and_then(Value::as_str)
                .map(String::from),
            time_max: args
                .get("time_max")
                .and_then(Value::as_str)
                .map(String::from),
            max_results: args
                .get("max_results")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            single_events: true,
            order_by: Some("startTime".to_string()),
            ..Default::default()
        };

        let events = list_events(&self.client, &self.access_token, &params)
            .await
            .map_err(|e| ZeptoError::Tool(format!("Calendar list failed: {}", e)))?;

        if events.items.is_empty() {
            return Ok("No events found.".to_string());
        }

        let mut lines = Vec::new();
        lines.push(format!("Found {} event(s):", events.items.len()));
        for event in &events.items {
            let start = event
                .start
                .as_ref()
                .and_then(|s| s.date_time.as_deref().or(s.date.as_deref()))
                .unwrap_or("unknown");
            let end = event
                .end
                .as_ref()
                .and_then(|e| e.date_time.as_deref().or(e.date.as_deref()))
                .unwrap_or("unknown");
            lines.push(format!(
                "  [{}] {} → {}  ID: {}",
                event.display_summary(),
                start,
                end,
                event.id.as_deref().unwrap_or("?")
            ));
            if let Some(loc) = &event.location {
                lines.push(format!("      Location: {}", loc));
            }
        }

        Ok(lines.join("\n"))
    }

    async fn calendar_create(&self, args: &Value) -> Result<String> {
        let summary = args
            .get("summary")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'summary' for calendar_create".to_string()))?;

        let start = args
            .get("start")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'start' for calendar_create".to_string()))?;

        let end = args
            .get("end")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'end' for calendar_create".to_string()))?;

        let calendar_id = args
            .get("calendar_id")
            .and_then(Value::as_str)
            .unwrap_or(&self.default_calendar)
            .to_string();

        let attendees = args
            .get("attendees")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let params = CreateParams {
            calendar_id,
            summary: summary.to_string(),
            description: args
                .get("description")
                .and_then(Value::as_str)
                .map(String::from),
            location: args
                .get("location")
                .and_then(Value::as_str)
                .map(String::from),
            start: EventDateTime::date_time(start, None),
            end: EventDateTime::date_time(end, None),
            attendees,
            recurrence: vec![],
        };

        let event = create_event(&self.client, &self.access_token, &params)
            .await
            .map_err(|e| ZeptoError::Tool(format!("Calendar create failed: {}", e)))?;

        let mut lines = Vec::new();
        lines.push("Event created successfully.".to_string());
        lines.push(format!("  Title: {}", event.display_summary()));
        if let Some(id) = &event.id {
            lines.push(format!("  Event ID: {}", id));
        }
        if let Some(link) = &event.html_link {
            lines.push(format!("  Link: {}", link));
        }

        Ok(lines.join("\n"))
    }

    async fn calendar_freebusy(&self, args: &Value) -> Result<String> {
        let time_min = args
            .get("time_min")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ZeptoError::Tool("Missing 'time_min' for calendar_freebusy".to_string())
            })?;

        let time_max = args
            .get("time_max")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ZeptoError::Tool("Missing 'time_max' for calendar_freebusy".to_string())
            })?;

        let calendars: Vec<String> = args
            .get("calendars")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_else(|| vec![self.default_calendar.clone()]);

        let result = query_freebusy(
            &self.client,
            &self.access_token,
            &calendars,
            time_min,
            time_max,
        )
        .await
        .map_err(|e| ZeptoError::Tool(format!("Calendar freebusy failed: {}", e)))?;

        let mut lines = Vec::new();
        lines.push(format!(
            "Free/busy query from {} to {}:",
            result.time_min.as_deref().unwrap_or(time_min),
            result.time_max.as_deref().unwrap_or(time_max)
        ));

        match &result.calendars {
            Some(cals) if cals.is_object() => {
                for cal_id in &calendars {
                    if let Some(cal_data) = cals.get(cal_id) {
                        let busy_slots = cal_data
                            .get("busy")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        if busy_slots.is_empty() {
                            lines.push(format!("  {}: FREE (no busy slots)", cal_id));
                        } else {
                            lines.push(format!("  {}: {} busy slot(s)", cal_id, busy_slots.len()));
                            for slot in &busy_slots {
                                let slot_start =
                                    slot.get("start").and_then(Value::as_str).unwrap_or("?");
                                let slot_end =
                                    slot.get("end").and_then(Value::as_str).unwrap_or("?");
                                lines.push(format!("    {} → {}", slot_start, slot_end));
                            }
                        }
                    }
                }
            }
            Some(cals) => {
                lines.push(format!("Raw calendars data: {}", cals));
            }
            None => {
                lines.push("No calendar data returned.".to_string());
            }
        }

        Ok(lines.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let tool = GoogleTool::new("ya29.test", "primary", 20);
        assert_eq!(tool.access_token, "ya29.test");
        assert_eq!(tool.default_calendar, "primary");
        assert_eq!(tool.max_search_results, 20);
    }

    #[test]
    fn test_name() {
        let tool = GoogleTool::new("t", "primary", 20);
        assert_eq!(tool.name(), "google");
    }

    #[test]
    fn test_description_contains_actions() {
        let tool = GoogleTool::new("t", "primary", 20);
        assert!(tool.description().contains("gmail_search"));
        assert!(tool.description().contains("calendar_freebusy"));
    }

    #[test]
    fn test_compact_description() {
        let tool = GoogleTool::new("t", "primary", 20);
        assert_eq!(tool.compact_description(), "Gmail+Calendar");
    }

    #[test]
    fn test_category() {
        let tool = GoogleTool::new("t", "primary", 20);
        assert_eq!(tool.category(), ToolCategory::Messaging);
    }

    #[test]
    fn test_is_dangerous_action_send() {
        assert!(GoogleTool::is_dangerous_action("gmail_send"));
        assert!(GoogleTool::is_dangerous_action("gmail_reply"));
        assert!(GoogleTool::is_dangerous_action("calendar_create"));
    }

    #[test]
    fn test_is_dangerous_action_safe() {
        assert!(!GoogleTool::is_dangerous_action("gmail_search"));
        assert!(!GoogleTool::is_dangerous_action("gmail_read"));
        assert!(!GoogleTool::is_dangerous_action("calendar_list"));
        assert!(!GoogleTool::is_dangerous_action("calendar_freebusy"));
    }

    #[test]
    fn test_parameters_has_action() {
        let tool = GoogleTool::new("t", "primary", 20);
        let params = tool.parameters();
        let props = params.get("properties").unwrap();
        assert!(props.get("action").is_some());
    }

    #[test]
    fn test_parameters_action_enum() {
        let tool = GoogleTool::new("t", "primary", 20);
        let params = tool.parameters();
        let action_enum = params["properties"]["action"]["enum"].as_array().unwrap();
        assert_eq!(action_enum.len(), 7);
    }

    #[test]
    fn test_parameters_required_has_action() {
        let tool = GoogleTool::new("t", "primary", 20);
        let params = tool.parameters();
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("action")));
    }

    #[tokio::test]
    async fn test_missing_action() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'action'"));
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool.execute(json!({"action": "unknown"}), &ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown action 'unknown'"));
    }

    #[tokio::test]
    async fn test_gmail_search_missing_query() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool.execute(json!({"action": "gmail_search"}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'query'"));
    }

    #[tokio::test]
    async fn test_gmail_read_missing_message_id() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool.execute(json!({"action": "gmail_read"}), &ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing 'message_id'"));
    }

    #[tokio::test]
    async fn test_gmail_send_missing_to() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool
            .execute(
                json!({"action": "gmail_send", "subject": "hi", "body": "hello"}),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'to'"));
    }

    #[tokio::test]
    async fn test_gmail_send_missing_subject() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool
            .execute(
                json!({"action": "gmail_send", "to": "a@b.com", "body": "hello"}),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing 'subject'"));
    }

    #[tokio::test]
    async fn test_gmail_send_missing_body() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool
            .execute(
                json!({"action": "gmail_send", "to": "a@b.com", "subject": "hi"}),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'body'"));
    }

    #[tokio::test]
    async fn test_gmail_reply_missing_thread_id() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool
            .execute(
                json!({"action": "gmail_reply", "to": "a@b.com", "subject": "re", "body": "ok"}),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing 'thread_id'"));
    }

    #[tokio::test]
    async fn test_calendar_create_missing_summary() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool
            .execute(
                json!({
                    "action": "calendar_create",
                    "start": "2026-03-01T10:00:00Z",
                    "end": "2026-03-01T11:00:00Z"
                }),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing 'summary'"));
    }

    #[tokio::test]
    async fn test_calendar_create_missing_start() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool
            .execute(
                json!({
                    "action": "calendar_create",
                    "summary": "Meeting",
                    "end": "2026-03-01T11:00:00Z"
                }),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'start'"));
    }

    #[tokio::test]
    async fn test_calendar_freebusy_missing_time_min() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool
            .execute(
                json!({"action": "calendar_freebusy", "time_max": "2026-03-01T23:59:59Z"}),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing 'time_min'"));
    }

    #[tokio::test]
    async fn test_calendar_freebusy_missing_time_max() {
        let tool = GoogleTool::new("t", "primary", 20);
        let ctx = ToolContext::default();
        let result = tool
            .execute(
                json!({"action": "calendar_freebusy", "time_min": "2026-03-01T00:00:00Z"}),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing 'time_max'"));
    }
}
