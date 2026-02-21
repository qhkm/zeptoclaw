//! Stripe payment integration tool with production hardening.
//!
//! Provides a `StripeTool` for interacting with the Stripe API. Features:
//!
//! - **Idempotency keys**: All `create_payment` calls send a unique `Idempotency-Key`
//!   header to prevent duplicate charges on network retries.
//! - **Webhook signature verification**: Validates Stripe-Signature header using
//!   HMAC-SHA256 with timing-safe comparison and timestamp tolerance (5 minutes).
//! - **Rate limit handling**: Automatically sleeps for the `Retry-After` duration
//!   (capped at 30s) and retries once on HTTP 429 responses.
//!
//! ## Supported actions
//!
//! - `create_payment` — Create a PaymentIntent
//! - `get_payment` — Get a PaymentIntent by ID
//! - `list_payments` — List recent PaymentIntents
//! - `create_customer` — Create a Customer
//! - `get_customer` — Get a Customer by ID
//! - `list_customers` — List recent Customers
//! - `create_refund` — Refund a charge or PaymentIntent
//! - `get_balance` — Retrieve current account balance
//! - `verify_webhook` — Verify a Stripe webhook signature (HMAC-SHA256 + timestamp)

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use sha2::Digest;

use crate::error::{Result, ZeptoError};

use super::{Tool, ToolContext};

/// Monotonically-increasing counter for idempotency key disambiguation.
static IDEM_KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

const STRIPE_API_BASE: &str = "https://api.stripe.com/v1";

// ---------------------------------------------------------------------------
// HMAC-SHA256 implementation using the sha2 crate
// ---------------------------------------------------------------------------

/// Block size for SHA-256 in bytes.
const SHA256_BLOCK_SIZE: usize = 64;
/// Output size for SHA-256 in bytes.
const SHA256_OUTPUT_SIZE: usize = 32;

/// Compute HMAC-SHA256(key, message) as a lowercase hex string.
///
/// Implements RFC 2104 HMAC using the sha2 crate's SHA-256 implementation.
/// No external hmac crate required.
fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    // Normalise key: hash if longer than block size, pad to block size otherwise.
    let mut k = [0u8; SHA256_BLOCK_SIZE];
    if key.len() > SHA256_BLOCK_SIZE {
        let hashed = sha2::Sha256::digest(key);
        k[..SHA256_OUTPUT_SIZE].copy_from_slice(&hashed);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    // Derive ipad and opad masked keys.
    let mut k_ipad = [0u8; SHA256_BLOCK_SIZE];
    let mut k_opad = [0u8; SHA256_BLOCK_SIZE];
    for i in 0..SHA256_BLOCK_SIZE {
        k_ipad[i] = k[i] ^ 0x36;
        k_opad[i] = k[i] ^ 0x5c;
    }

    // Inner hash: SHA256(k_ipad || message)
    let mut inner = sha2::Sha256::new();
    inner.update(k_ipad);
    inner.update(message);
    let inner_result = inner.finalize();

    // Outer hash: SHA256(k_opad || inner_hash)
    let mut outer = sha2::Sha256::new();
    outer.update(k_opad);
    outer.update(inner_result);
    let mac = outer.finalize();

    // Encode as lowercase hex.
    hex::encode(mac)
}

/// Constant-time byte-slice comparison to resist timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Idempotency key
// ---------------------------------------------------------------------------

/// Generate a unique idempotency key using nanosecond timestamp + process ID +
/// a monotonically-increasing counter.
///
/// The counter ensures keys are unique even when multiple keys are generated
/// within the same nanosecond (e.g. in unit tests). No external uuid crate required.
fn generate_idempotency_key() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let seq = IDEM_KEY_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("zc_{:x}_{:x}_{:x}", ts, pid, seq)
}

// ---------------------------------------------------------------------------
// StripeTool
// ---------------------------------------------------------------------------

/// Tool for Stripe payment API operations with production hardening.
pub struct StripeTool {
    /// Stripe secret key (sk_live_... or sk_test_...).
    secret_key: String,
    /// Default ISO 4217 currency code (e.g., "usd", "myr", "sgd").
    default_currency: String,
    /// Optional webhook signing secret for signature verification.
    webhook_secret: Option<String>,
    /// Reqwest HTTP client.
    client: Client,
}

impl StripeTool {
    /// Create a new StripeTool from explicit credentials.
    pub fn new(secret_key: &str, default_currency: &str) -> Self {
        Self {
            secret_key: secret_key.to_string(),
            default_currency: default_currency.to_string(),
            webhook_secret: None,
            client: Client::new(),
        }
    }

    /// Create with a webhook signing secret for `verify_webhook` support.
    pub fn with_webhook_secret(mut self, webhook_secret: &str) -> Self {
        self.webhook_secret = Some(webhook_secret.to_string());
        self
    }

    /// Create from the global ZeptoClaw configuration.
    ///
    /// Returns an error if `stripe.secret_key` is not configured.
    pub fn from_config() -> Result<Self> {
        let config = crate::config::Config::get();
        let stripe_cfg = &config.stripe;

        let secret_key = stripe_cfg
            .secret_key
            .as_deref()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                ZeptoError::Tool(
                    "stripe.secret_key not configured; set it in config.json or \
                     ZEPTOCLAW_STRIPE_SECRET_KEY"
                        .into(),
                )
            })?;

        Ok(Self {
            secret_key: secret_key.to_string(),
            default_currency: stripe_cfg.default_currency.clone(),
            webhook_secret: stripe_cfg.webhook_secret.clone(),
            client: Client::new(),
        })
    }

    // -----------------------------------------------------------------------
    // HTTP helper with rate-limit retry
    // -----------------------------------------------------------------------

    /// Execute a Stripe API request, retrying once on HTTP 429 (rate limited).
    ///
    /// On 429, sleeps for the `Retry-After` header value (capped at 30s) before
    /// retrying. API errors (4xx/5xx other than 429) are propagated as
    /// `ZeptoError::Tool`.
    async fn stripe_request_with_retry(
        &self,
        method: reqwest::Method,
        url: &str,
        form: Option<Vec<(&str, String)>>,
        idempotency_key: Option<&str>,
    ) -> Result<Value> {
        let resp = self
            .execute_request(method.clone(), url, form.clone(), idempotency_key)
            .await?;

        if resp.status() == 429 {
            // Rate limited — honour Retry-After (capped at 30 seconds).
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(1)
                .min(30);

            tracing::warn!(
                retry_after_secs = retry_after,
                "Stripe rate limit hit (429); backing off"
            );
            tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;

            // Retry once.
            let resp2 = self
                .execute_request(method, url, form, idempotency_key)
                .await?;
            return self.parse_stripe_response(resp2).await;
        }

        self.parse_stripe_response(resp).await
    }

    /// Build and send a single HTTP request to Stripe.
    async fn execute_request(
        &self,
        method: reqwest::Method,
        url: &str,
        form: Option<Vec<(&str, String)>>,
        idempotency_key: Option<&str>,
    ) -> Result<reqwest::Response> {
        let mut builder = self
            .client
            .request(method, url)
            .basic_auth(&self.secret_key, None::<&str>);

        if let Some(key) = idempotency_key {
            builder = builder.header("Idempotency-Key", key);
        }

        if let Some(params) = form {
            builder = builder.form(&params);
        }

        builder
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Stripe HTTP error: {}", e)))
    }

    /// Parse a Stripe HTTP response into a JSON `Value`.
    ///
    /// On non-2xx responses, extracts and surfaces the Stripe error message.
    async fn parse_stripe_response(&self, resp: reqwest::Response) -> Result<Value> {
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to parse Stripe response: {}", e)))?;

        if !status.is_success() {
            let error_msg = body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown Stripe error");
            return Err(ZeptoError::Tool(format!(
                "Stripe API error ({}): {}",
                status, error_msg
            )));
        }

        Ok(body)
    }

    // -----------------------------------------------------------------------
    // Action implementations
    // -----------------------------------------------------------------------

    async fn create_payment(&self, args: &Value) -> Result<String> {
        let amount = args
            .get("amount")
            .and_then(Value::as_i64)
            .ok_or_else(|| ZeptoError::Tool("Missing 'amount' parameter (integer cents)".into()))?;

        if amount <= 0 {
            return Err(ZeptoError::Tool(
                "'amount' must be a positive integer (smallest currency unit)".into(),
            ));
        }

        let currency = args
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or(&self.default_currency)
            .to_ascii_lowercase();

        let description = args
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let url = format!("{}/payment_intents", STRIPE_API_BASE);
        let mut form: Vec<(&str, String)> =
            vec![("amount", amount.to_string()), ("currency", currency)];
        if !description.is_empty() {
            form.push(("description", description));
        }

        // Idempotency key prevents duplicate charges on network retries.
        let idem_key = generate_idempotency_key();

        let data = self
            .stripe_request_with_retry(reqwest::Method::POST, &url, Some(form), Some(&idem_key))
            .await?;

        let id = data.get("id").and_then(Value::as_str).unwrap_or("unknown");
        let status = data
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let client_secret = data
            .get("client_secret")
            .and_then(Value::as_str)
            .unwrap_or("(none)");

        Ok(format!(
            "PaymentIntent created. id={} status={} client_secret={} idempotency_key={}",
            id, status, client_secret, idem_key
        ))
    }

    async fn get_payment(&self, args: &Value) -> Result<String> {
        let id = args
            .get("payment_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'payment_id' parameter".into()))?;

        let url = format!("{}/payment_intents/{}", STRIPE_API_BASE, id);
        let data = self
            .stripe_request_with_retry(reqwest::Method::GET, &url, None, None)
            .await?;

        Ok(serde_json::to_string_pretty(&data)
            .unwrap_or_else(|_| "Failed to format response".to_string()))
    }

    async fn list_payments(&self, args: &Value) -> Result<String> {
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .min(100);

        let url = format!("{}/payment_intents?limit={}", STRIPE_API_BASE, limit);
        let data = self
            .stripe_request_with_retry(reqwest::Method::GET, &url, None, None)
            .await?;

        let items = data
            .get("data")
            .and_then(Value::as_array)
            .map(|arr| arr.len())
            .unwrap_or(0);

        Ok(format!(
            "Found {} payment intent(s):\n{}",
            items,
            serde_json::to_string_pretty(&data).unwrap_or_default()
        ))
    }

    async fn create_customer(&self, args: &Value) -> Result<String> {
        let email = args
            .get("email")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'email' parameter".into()))?;

        let name = args
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let url = format!("{}/customers", STRIPE_API_BASE);
        let mut form: Vec<(&str, String)> = vec![("email", email.to_string())];
        if !name.is_empty() {
            form.push(("name", name));
        }

        let idem_key = generate_idempotency_key();
        let data = self
            .stripe_request_with_retry(reqwest::Method::POST, &url, Some(form), Some(&idem_key))
            .await?;

        let id = data.get("id").and_then(Value::as_str).unwrap_or("unknown");

        Ok(format!(
            "Customer created. id={} email={} idempotency_key={}",
            id, email, idem_key
        ))
    }

    async fn get_customer(&self, args: &Value) -> Result<String> {
        let id = args
            .get("customer_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'customer_id' parameter".into()))?;

        let url = format!("{}/customers/{}", STRIPE_API_BASE, id);
        let data = self
            .stripe_request_with_retry(reqwest::Method::GET, &url, None, None)
            .await?;

        Ok(serde_json::to_string_pretty(&data)
            .unwrap_or_else(|_| "Failed to format response".to_string()))
    }

    async fn list_customers(&self, args: &Value) -> Result<String> {
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .min(100);

        let url = format!("{}/customers?limit={}", STRIPE_API_BASE, limit);
        let data = self
            .stripe_request_with_retry(reqwest::Method::GET, &url, None, None)
            .await?;

        let items = data
            .get("data")
            .and_then(Value::as_array)
            .map(|arr| arr.len())
            .unwrap_or(0);

        Ok(format!(
            "Found {} customer(s):\n{}",
            items,
            serde_json::to_string_pretty(&data).unwrap_or_default()
        ))
    }

    async fn create_refund(&self, args: &Value) -> Result<String> {
        let payment_intent_id = args
            .get("payment_intent_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'payment_intent_id' parameter".into()))?;

        let url = format!("{}/refunds", STRIPE_API_BASE);
        let mut form: Vec<(&str, String)> = vec![("payment_intent", payment_intent_id.to_string())];

        if let Some(amount) = args.get("amount").and_then(Value::as_i64) {
            if amount > 0 {
                form.push(("amount", amount.to_string()));
            }
        }

        let idem_key = generate_idempotency_key();
        let data = self
            .stripe_request_with_retry(reqwest::Method::POST, &url, Some(form), Some(&idem_key))
            .await?;

        let id = data.get("id").and_then(Value::as_str).unwrap_or("unknown");
        let status = data
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        Ok(format!(
            "Refund created. id={} status={} idempotency_key={}",
            id, status, idem_key
        ))
    }

    async fn get_balance(&self) -> Result<String> {
        let url = format!("{}/balance", STRIPE_API_BASE);
        let data = self
            .stripe_request_with_retry(reqwest::Method::GET, &url, None, None)
            .await?;

        Ok(serde_json::to_string_pretty(&data)
            .unwrap_or_else(|_| "Failed to format response".to_string()))
    }

    /// Verify a Stripe webhook signature.
    ///
    /// Stripe sends a `Stripe-Signature` header of the form:
    /// `t=<unix_ts>,v1=<hmac_hex>`
    ///
    /// This method:
    /// 1. Parses `t=` (timestamp) and `v1=` (HMAC-SHA256 hex) from the header.
    /// 2. Rejects events whose timestamp is more than 5 minutes old or in the future
    ///    to prevent replay attacks.
    /// 3. Computes `HMAC-SHA256(webhook_secret, "<timestamp>.<payload>")` and
    ///    performs a **constant-time** comparison with the received signature.
    async fn verify_webhook(&self, args: &Value) -> Result<String> {
        let payload = args
            .get("payload")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'payload' parameter".into()))?;

        let signature = args
            .get("signature")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'signature' parameter".into()))?;

        let webhook_secret = self
            .webhook_secret
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ZeptoError::Tool(
                    "stripe.webhook_secret not configured; set it in config.json or \
                     ZEPTOCLAW_STRIPE_WEBHOOK_SECRET"
                        .into(),
                )
            })?;

        // Parse Stripe-Signature header: t=<ts>,v1=<hex>
        let mut timestamp_str: Option<&str> = None;
        let mut sig_v1: Option<&str> = None;
        for part in signature.split(',') {
            let part = part.trim();
            if let Some(val) = part.strip_prefix("t=") {
                timestamp_str = Some(val);
            } else if let Some(val) = part.strip_prefix("v1=") {
                sig_v1 = Some(val);
            }
        }

        let ts_str = timestamp_str.ok_or_else(|| {
            ZeptoError::Tool("Invalid Stripe-Signature header: missing t= timestamp field".into())
        })?;

        let received_sig = sig_v1.ok_or_else(|| {
            ZeptoError::Tool("Invalid Stripe-Signature header: missing v1= signature field".into())
        })?;

        // Validate timestamp (reject events older than 5 minutes or 60s in future).
        let ts_secs: u64 = ts_str
            .parse()
            .map_err(|_| ZeptoError::Tool("Invalid timestamp in Stripe-Signature header".into()))?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let tolerance_secs: u64 = 300; // 5 minutes
        if now.abs_diff(ts_secs) > tolerance_secs {
            return Err(ZeptoError::Tool(format!(
                "Webhook timestamp rejected: {}s delta exceeds {}s tolerance (replay attack or clock skew)",
                now.abs_diff(ts_secs),
                tolerance_secs
            )));
        }

        // Compute expected HMAC-SHA256.
        // Stripe signed_payload = "<timestamp>.<body>"
        let signed_payload = format!("{}.{}", ts_str, payload);
        let expected_sig = hmac_sha256_hex(webhook_secret.as_bytes(), signed_payload.as_bytes());

        // Constant-time comparison to prevent timing attacks.
        if !constant_time_eq(received_sig.as_bytes(), expected_sig.as_bytes()) {
            return Err(ZeptoError::Tool(
                "Webhook signature verification failed: HMAC mismatch".into(),
            ));
        }

        Ok(format!(
            "Webhook signature verified. timestamp={} ({}s ago)",
            ts_str,
            now.saturating_sub(ts_secs)
        ))
    }
}

// ---------------------------------------------------------------------------
// Tool trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for StripeTool {
    fn name(&self) -> &str {
        "stripe"
    }

    fn description(&self) -> &str {
        "Interact with the Stripe payment API. Supports creating and retrieving \
         PaymentIntents, Customers, Refunds, and balance. Also verifies Stripe \
         webhook signatures (HMAC-SHA256). Requires stripe.secret_key in config."
    }

    fn compact_description(&self) -> &str {
        "Stripe payments: create/get payment, customer, refund, balance, verify webhook"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "The Stripe operation to perform.",
                    "enum": [
                        "create_payment",
                        "get_payment",
                        "list_payments",
                        "create_customer",
                        "get_customer",
                        "list_customers",
                        "create_refund",
                        "get_balance",
                        "verify_webhook"
                    ]
                },
                "amount": {
                    "type": "integer",
                    "description": "Amount in smallest currency unit (e.g. cents for USD). Required for create_payment."
                },
                "currency": {
                    "type": "string",
                    "description": "ISO 4217 currency code (e.g. 'usd', 'myr'). Defaults to config default_currency."
                },
                "description": {
                    "type": "string",
                    "description": "Optional description for create_payment."
                },
                "payment_id": {
                    "type": "string",
                    "description": "PaymentIntent ID (pi_...). Required for get_payment."
                },
                "payment_intent_id": {
                    "type": "string",
                    "description": "PaymentIntent ID to refund. Required for create_refund."
                },
                "customer_id": {
                    "type": "string",
                    "description": "Customer ID (cus_...). Required for get_customer."
                },
                "email": {
                    "type": "string",
                    "description": "Customer email address. Required for create_customer."
                },
                "name": {
                    "type": "string",
                    "description": "Customer name. Optional for create_customer."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results for list operations (1-100, default 10)."
                },
                "payload": {
                    "type": "string",
                    "description": "Raw webhook request body. Required for verify_webhook."
                },
                "signature": {
                    "type": "string",
                    "description": "Stripe-Signature header value. Required for verify_webhook."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<String> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'action' parameter".into()))?;

        match action {
            "create_payment" => self.create_payment(&args).await,
            "get_payment" => self.get_payment(&args).await,
            "list_payments" => self.list_payments(&args).await,
            "create_customer" => self.create_customer(&args).await,
            "get_customer" => self.get_customer(&args).await,
            "list_customers" => self.list_customers(&args).await,
            "create_refund" => self.create_refund(&args).await,
            "get_balance" => self.get_balance().await,
            "verify_webhook" => self.verify_webhook(&args).await,
            other => Err(ZeptoError::Tool(format!(
                "Unknown stripe action '{}'. Valid actions: create_payment, get_payment, \
                 list_payments, create_customer, get_customer, list_customers, \
                 create_refund, get_balance, verify_webhook",
                other
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // Config & constructor tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_stripe_config_defaults() {
        let cfg = crate::config::StripeConfig::default();
        assert!(cfg.secret_key.is_none());
        assert_eq!(cfg.default_currency, "usd");
        assert!(cfg.webhook_secret.is_none());
    }

    #[test]
    fn test_stripe_config_deserialize() {
        let json = r#"{
            "secret_key": "sk_test_abc",
            "default_currency": "myr",
            "webhook_secret": "whsec_xyz"
        }"#;
        let cfg: crate::config::StripeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.secret_key.as_deref(), Some("sk_test_abc"));
        assert_eq!(cfg.default_currency, "myr");
        assert_eq!(cfg.webhook_secret.as_deref(), Some("whsec_xyz"));
    }

    #[test]
    fn test_stripe_config_deserialize_minimal() {
        // Only secret_key, rest defaults.
        let json = r#"{"secret_key": "sk_test_xyz"}"#;
        let cfg: crate::config::StripeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.secret_key.as_deref(), Some("sk_test_xyz"));
        assert_eq!(cfg.default_currency, "usd");
        assert!(cfg.webhook_secret.is_none());
    }

    #[test]
    fn test_stripe_config_webhook_secret_present() {
        let cfg = crate::config::StripeConfig {
            secret_key: Some("sk_test_key".to_string()),
            default_currency: "sgd".to_string(),
            webhook_secret: Some("whsec_secret".to_string()),
        };
        assert!(cfg.webhook_secret.is_some());
    }

    #[test]
    fn test_config_stripe_field_in_top_level() {
        // Verify stripe field is recognized at the top level of Config.
        let json = r#"{"stripe": {"secret_key": "sk_test_abc", "default_currency": "myr"}}"#;
        let config: crate::config::Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.stripe.secret_key.as_deref(), Some("sk_test_abc"));
        assert_eq!(config.stripe.default_currency, "myr");
    }

    #[test]
    fn test_stripe_tool_constructor() {
        let tool = StripeTool::new("sk_test_abc", "usd");
        assert_eq!(tool.name(), "stripe");
        assert_eq!(tool.secret_key, "sk_test_abc");
        assert_eq!(tool.default_currency, "usd");
        assert!(tool.webhook_secret.is_none());
    }

    #[test]
    fn test_stripe_tool_with_webhook_secret() {
        let tool = StripeTool::new("sk_test_abc", "usd").with_webhook_secret("whsec_test_secret");
        assert_eq!(tool.webhook_secret.as_deref(), Some("whsec_test_secret"));
    }

    #[test]
    fn test_stripe_tool_metadata() {
        let tool = StripeTool::new("sk_test_abc", "usd");
        assert_eq!(tool.name(), "stripe");
        assert!(!tool.description().is_empty());
        assert!(!tool.compact_description().is_empty());
        // Compact description should be shorter than full description.
        assert!(
            tool.compact_description().len() < tool.description().len(),
            "compact_description should be shorter than description"
        );
    }

    #[test]
    fn test_stripe_tool_parameters_schema() {
        let tool = StripeTool::new("sk_test_abc", "usd");
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
        assert_eq!(params["required"], json!(["action"]));
        // Verify all actions are listed in the enum.
        let actions = params["properties"]["action"]["enum"].as_array().unwrap();
        let action_strs: Vec<&str> = actions.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(action_strs.contains(&"create_payment"));
        assert!(action_strs.contains(&"verify_webhook"));
        assert!(action_strs.contains(&"get_balance"));
    }

    // -----------------------------------------------------------------------
    // Idempotency key tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_idempotency_key_format() {
        let key = generate_idempotency_key();
        assert!(
            key.starts_with("zc_"),
            "key should start with 'zc_': {}",
            key
        );
        // Should have three hex components after the 'zc' prefix: ts, pid, seq.
        let parts: Vec<&str> = key.splitn(4, '_').collect();
        assert_eq!(
            parts.len(),
            4,
            "key should have format zc_<ts>_<pid>_<seq>: {}",
            key
        );
        // All three components should be valid hex strings.
        assert!(
            u128::from_str_radix(parts[1], 16).is_ok(),
            "timestamp part should be hex: {}",
            parts[1]
        );
        assert!(
            u32::from_str_radix(parts[2], 16).is_ok(),
            "pid part should be hex: {}",
            parts[2]
        );
        assert!(
            u64::from_str_radix(parts[3], 16).is_ok(),
            "sequence part should be hex: {}",
            parts[3]
        );
    }

    #[test]
    fn test_idempotency_key_uniqueness() {
        // Generate multiple keys and verify they're unique.
        let mut keys: Vec<String> = (0..20).map(|_| generate_idempotency_key()).collect();
        keys.sort();
        keys.dedup();
        // Allow for up to 1 collision given nanosecond resolution, but all should be unique.
        assert!(
            keys.len() >= 19,
            "expected at least 19/20 unique keys, got {}",
            keys.len()
        );
    }

    #[test]
    fn test_idempotency_key_not_empty() {
        let key = generate_idempotency_key();
        assert!(!key.is_empty());
        assert!(key.len() > 5);
    }

    // -----------------------------------------------------------------------
    // HMAC-SHA256 tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_hmac_sha256_known_vector() {
        // RFC 2202 Test Case 1:
        // Key = 0x0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b (20 bytes)
        // Data = "Hi There"
        // Expected HMAC-SHA256 = b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7
        let key = [0x0bu8; 20];
        let data = b"Hi There";
        let result = hmac_sha256_hex(&key, data);
        assert_eq!(
            result, "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            "HMAC-SHA256 RFC 2202 test vector 1 failed"
        );
    }

    #[test]
    fn test_hmac_sha256_key_larger_than_block() {
        // Key longer than 64 bytes should be hashed first.
        let key = vec![0xaau8; 131]; // 131 bytes > 64-byte block
        let data = b"Test With Truncation";
        // Just verify it doesn't panic and returns 64-char hex.
        let result = hmac_sha256_hex(&key, data);
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn test_hmac_sha256_different_inputs_produce_different_outputs() {
        let key = b"test-key";
        let h1 = hmac_sha256_hex(key, b"message1");
        let h2 = hmac_sha256_hex(key, b"message2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hmac_sha256_same_inputs_deterministic() {
        let key = b"secret";
        let msg = b"payload";
        let h1 = hmac_sha256_hex(key, msg);
        let h2 = hmac_sha256_hex(key, msg);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_constant_time_eq_equal() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn test_constant_time_eq_different() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"hello", b"hell"));
    }

    #[test]
    fn test_constant_time_eq_empty() {
        assert!(constant_time_eq(b"", b""));
    }

    // -----------------------------------------------------------------------
    // Webhook verification tests
    // -----------------------------------------------------------------------

    /// Build a valid Stripe-Signature header for testing.
    fn make_stripe_signature(secret: &str, payload: &str, ts: u64) -> String {
        let signed_payload = format!("{}.{}", ts, payload);
        let sig = hmac_sha256_hex(secret.as_bytes(), signed_payload.as_bytes());
        format!("t={},v1={}", ts, sig)
    }

    #[tokio::test]
    async fn test_verify_webhook_valid() {
        let secret = "whsec_test_secret";
        let payload = r#"{"id":"evt_123","type":"payment_intent.succeeded"}"#;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let sig = make_stripe_signature(secret, payload, now);

        let tool = StripeTool::new("sk_test_abc", "usd").with_webhook_secret(secret);
        let args = json!({"action": "verify_webhook", "payload": payload, "signature": sig});

        let result = tool.verify_webhook(&args).await;
        assert!(result.is_ok(), "Expected success but got: {:?}", result);
        let msg = result.unwrap();
        assert!(msg.contains("verified"), "Expected 'verified' in: {}", msg);
    }

    #[tokio::test]
    async fn test_verify_webhook_missing_payload() {
        let tool = StripeTool::new("sk_test_abc", "usd").with_webhook_secret("whsec_x");
        let args = json!({"action": "verify_webhook", "signature": "t=123,v1=abc"});
        let result = tool.verify_webhook(&args).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("payload"));
    }

    #[tokio::test]
    async fn test_verify_webhook_missing_signature() {
        let tool = StripeTool::new("sk_test_abc", "usd").with_webhook_secret("whsec_x");
        let args = json!({"action": "verify_webhook", "payload": "body"});
        let result = tool.verify_webhook(&args).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("signature"));
    }

    #[tokio::test]
    async fn test_verify_webhook_no_secret_configured() {
        // Tool without webhook_secret.
        let tool = StripeTool::new("sk_test_abc", "usd");
        let args = json!({
            "action": "verify_webhook",
            "payload": "body",
            "signature": "t=123,v1=abc"
        });
        let result = tool.verify_webhook(&args).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("webhook_secret") || err.contains("not configured"),
            "Expected webhook_secret error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_verify_webhook_missing_timestamp_field() {
        let tool = StripeTool::new("sk_test_abc", "usd").with_webhook_secret("whsec_x");
        let args = json!({
            "action": "verify_webhook",
            "payload": "body",
            "signature": "v1=abc123"  // no t= field
        });
        let result = tool.verify_webhook(&args).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("t=") || err.contains("timestamp"),
            "Expected timestamp error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_verify_webhook_missing_v1_field() {
        let tool = StripeTool::new("sk_test_abc", "usd").with_webhook_secret("whsec_x");
        let args = json!({
            "action": "verify_webhook",
            "payload": "body",
            "signature": "t=1234567890"  // no v1= field
        });
        let result = tool.verify_webhook(&args).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("v1=") || err.contains("signature"),
            "Expected v1 signature error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_verify_webhook_expired_timestamp() {
        let tool = StripeTool::new("sk_test_abc", "usd").with_webhook_secret("whsec_x");
        // Timestamp 10 minutes in the past (exceeds 5-minute tolerance).
        let old_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_sub(601);

        let args = json!({
            "action": "verify_webhook",
            "payload": "body",
            "signature": format!("t={},v1=abc123", old_ts)
        });
        let result = tool.verify_webhook(&args).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timestamp") || err.contains("tolerance") || err.contains("delta"),
            "Expected timestamp expiry error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_verify_webhook_future_timestamp() {
        let tool = StripeTool::new("sk_test_abc", "usd").with_webhook_secret("whsec_x");
        // Timestamp 10 minutes in the future.
        let future_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 601;

        let args = json!({
            "action": "verify_webhook",
            "payload": "body",
            "signature": format!("t={},v1=abc123", future_ts)
        });
        let result = tool.verify_webhook(&args).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timestamp") || err.contains("tolerance") || err.contains("delta"),
            "Expected future timestamp error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_verify_webhook_wrong_signature() {
        let tool = StripeTool::new("sk_test_abc", "usd").with_webhook_secret("whsec_test_secret");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let args = json!({
            "action": "verify_webhook",
            "payload": "real payload",
            "signature": format!("t={},v1={}", now, "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
        });
        let result = tool.verify_webhook(&args).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("mismatch") || err.contains("failed") || err.contains("HMAC"),
            "Expected HMAC mismatch error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_verify_webhook_invalid_timestamp_not_numeric() {
        let tool = StripeTool::new("sk_test_abc", "usd").with_webhook_secret("whsec_x");
        let args = json!({
            "action": "verify_webhook",
            "payload": "body",
            "signature": "t=notanumber,v1=abc123"
        });
        let result = tool.verify_webhook(&args).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timestamp") || err.contains("Invalid"),
            "Expected timestamp parse error, got: {}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // Missing action / unknown action tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_unknown_action_returns_error() {
        let tool = StripeTool::new("sk_test_abc", "usd");
        let ctx = ToolContext::new();
        let result = tool
            .execute(json!({"action": "fly_to_the_moon"}), &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("fly_to_the_moon"));
    }

    #[tokio::test]
    async fn test_missing_action_returns_error() {
        let tool = StripeTool::new("sk_test_abc", "usd");
        let ctx = ToolContext::new();
        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("action"));
    }

    // -----------------------------------------------------------------------
    // create_payment validation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_payment_missing_amount_returns_error() {
        let tool = StripeTool::new("sk_test_abc", "usd");
        let ctx = ToolContext::new();
        let result = tool
            .execute(json!({"action": "create_payment", "currency": "usd"}), &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("amount"));
    }

    #[tokio::test]
    async fn test_create_payment_zero_amount_returns_error() {
        let tool = StripeTool::new("sk_test_abc", "usd");
        let ctx = ToolContext::new();
        let result = tool
            .execute(
                json!({"action": "create_payment", "amount": 0, "currency": "usd"}),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("positive"));
    }

    #[tokio::test]
    async fn test_create_payment_negative_amount_returns_error() {
        let tool = StripeTool::new("sk_test_abc", "usd");
        let ctx = ToolContext::new();
        let result = tool
            .execute(json!({"action": "create_payment", "amount": -100}), &ctx)
            .await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // get_payment validation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_payment_missing_id_returns_error() {
        let tool = StripeTool::new("sk_test_abc", "usd");
        let ctx = ToolContext::new();
        let result = tool.execute(json!({"action": "get_payment"}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("payment_id"));
    }

    // -----------------------------------------------------------------------
    // create_customer validation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_customer_missing_email_returns_error() {
        let tool = StripeTool::new("sk_test_abc", "usd");
        let ctx = ToolContext::new();
        let result = tool
            .execute(json!({"action": "create_customer"}), &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("email"));
    }

    // -----------------------------------------------------------------------
    // create_refund validation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_refund_missing_payment_intent_id_returns_error() {
        let tool = StripeTool::new("sk_test_abc", "usd");
        let ctx = ToolContext::new();
        let result = tool.execute(json!({"action": "create_refund"}), &ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("payment_intent_id"));
    }

    // -----------------------------------------------------------------------
    // Env override tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_stripe_env_overrides() {
        // Set env vars and load config via Config::load_from_path (uses a
        // nonexistent path so it starts from defaults, then applies env vars).
        std::env::set_var("ZEPTOCLAW_STRIPE_SECRET_KEY", "sk_test_env_key");
        std::env::set_var("ZEPTOCLAW_STRIPE_DEFAULT_CURRENCY", "SGD");
        std::env::set_var("ZEPTOCLAW_STRIPE_WEBHOOK_SECRET", "whsec_env_secret");

        let path = std::path::PathBuf::from("/nonexistent/stripe_test_config.json");
        let config = crate::config::Config::load_from_path(&path).unwrap();

        assert_eq!(config.stripe.secret_key.as_deref(), Some("sk_test_env_key"));
        assert_eq!(config.stripe.default_currency, "sgd"); // env var is lowercased
        assert_eq!(
            config.stripe.webhook_secret.as_deref(),
            Some("whsec_env_secret")
        );

        std::env::remove_var("ZEPTOCLAW_STRIPE_SECRET_KEY");
        std::env::remove_var("ZEPTOCLAW_STRIPE_DEFAULT_CURRENCY");
        std::env::remove_var("ZEPTOCLAW_STRIPE_WEBHOOK_SECRET");
    }

    #[test]
    fn test_stripe_config_in_config_json() {
        // Verify stripe fields round-trip through JSON serialization.
        let mut config = crate::config::Config::default();
        config.stripe = crate::config::StripeConfig {
            secret_key: Some("sk_test_round_trip".to_string()),
            default_currency: "eur".to_string(),
            webhook_secret: Some("whsec_round_trip".to_string()),
        };

        let json_str = serde_json::to_string(&config).unwrap();
        let loaded: crate::config::Config = serde_json::from_str(&json_str).unwrap();

        assert_eq!(
            loaded.stripe.secret_key.as_deref(),
            Some("sk_test_round_trip")
        );
        assert_eq!(loaded.stripe.default_currency, "eur");
        assert_eq!(
            loaded.stripe.webhook_secret.as_deref(),
            Some("whsec_round_trip")
        );
    }
}
