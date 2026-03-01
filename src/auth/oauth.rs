//! OAuth 2.0 + PKCE browser-based authentication flow.
//!
//! Implements the Authorization Code flow with PKCE (Proof Key for Code Exchange):
//! 1. Generate PKCE code_verifier + code_challenge
//! 2. Start local HTTP callback server on ephemeral port
//! 3. Open browser to provider's authorization URL
//! 4. Wait for callback with authorization code
//! 5. Exchange code for access + refresh tokens

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::error::{Result, ZeptoError};

use super::{OAuthTokenSet, ProviderOAuthConfig};

// ============================================================================
// PKCE Helpers
// ============================================================================

/// PKCE code verifier and challenge pair.
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    /// Random code verifier (43-128 chars, unreserved URI characters).
    pub code_verifier: String,
    /// SHA256 hash of verifier, base64url-encoded (no padding).
    pub code_challenge: String,
}

impl PkceChallenge {
    /// Generate a new PKCE challenge pair.
    pub fn generate() -> Self {
        use chacha20poly1305::aead::rand_core::RngCore;
        use chacha20poly1305::aead::OsRng;

        let mut buf = [0u8; 32];
        OsRng.fill_bytes(&mut buf);

        let code_verifier = URL_SAFE_NO_PAD.encode(buf);

        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let hash = hasher.finalize();
        let code_challenge = URL_SAFE_NO_PAD.encode(hash);

        Self {
            code_verifier,
            code_challenge,
        }
    }
}

// ============================================================================
// Callback Server
// ============================================================================

/// Result from the OAuth callback.
#[derive(Debug)]
pub struct CallbackResult {
    /// Authorization code returned by the provider.
    pub code: String,
    /// State parameter (must match the one sent in the auth request).
    pub state: Option<String>,
}

/// Start a local HTTP server to receive the OAuth callback.
///
/// Binds to `127.0.0.1:0` (ephemeral port) and waits for the provider
/// to redirect the browser with an authorization code.
///
/// Returns the callback result and the port the server listened on.
pub async fn start_callback_server(timeout_secs: u64) -> Result<(CallbackResult, u16)> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| ZeptoError::Config(format!("Failed to bind callback server: {}", e)))?;

    let port = listener
        .local_addr()
        .map_err(|e| ZeptoError::Config(format!("Failed to get callback server address: {}", e)))?
        .port();

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        accept_callback(&listener),
    )
    .await
    .map_err(|_| {
        ZeptoError::Config(format!(
            "OAuth callback timed out after {}s. Did you complete the browser sign-in?",
            timeout_secs
        ))
    })??;

    Ok((result, port))
}

/// Accept a single HTTP connection and extract the OAuth callback parameters.
async fn accept_callback(listener: &TcpListener) -> Result<CallbackResult> {
    let (mut stream, _addr) = listener
        .accept()
        .await
        .map_err(|e| ZeptoError::Config(format!("Failed to accept callback connection: {}", e)))?;

    let mut buf = vec![0u8; 4096];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| ZeptoError::Config(format!("Failed to read callback request: {}", e)))?;

    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse the GET request line to extract query parameters
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| ZeptoError::Config("Invalid callback HTTP request".into()))?;

    let result = parse_callback_params(path)?;

    // Send success response
    let html = r#"<!DOCTYPE html><html><body>
<h2>Authentication successful!</h2>
<p>You can close this tab and return to ZeptoClaw.</p>
<script>window.close();</script>
</body></html>"#;

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );

    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;

    Ok(result)
}

/// Parse OAuth callback query parameters from the request path.
fn parse_callback_params(path: &str) -> Result<CallbackResult> {
    let query = path
        .split('?')
        .nth(1)
        .ok_or_else(|| ZeptoError::Config("OAuth callback missing query parameters".into()))?;

    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_description = None;

    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        let key = kv.next().unwrap_or("");
        let value = kv.next().unwrap_or("");
        let value = urldecode(value);

        match key {
            "code" => code = Some(value),
            "state" => state = Some(value),
            "error" => error = Some(value),
            "error_description" => error_description = Some(value),
            _ => {}
        }
    }

    if let Some(err) = error {
        let desc = error_description.unwrap_or_default();
        return Err(ZeptoError::Config(format!(
            "OAuth authorization denied: {} — {}",
            err, desc
        )));
    }

    let code =
        code.ok_or_else(|| ZeptoError::Config("OAuth callback missing 'code' parameter".into()))?;

    Ok(CallbackResult { code, state })
}

/// Minimal URL decoding (percent-encoded characters).
fn urldecode(s: &str) -> String {
    // Decode into raw bytes first so multi-byte UTF-8 percent-encodings roundtrip correctly.
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 < bytes.len() {
                    let h1 = bytes[i + 1];
                    let h2 = bytes[i + 2];
                    let hex = [h1, h2];
                    if let Ok(hex_str) = std::str::from_utf8(&hex) {
                        if let Ok(byte) = u8::from_str_radix(hex_str, 16) {
                            out.push(byte);
                            i += 3;
                            continue;
                        }
                    }
                    out.push(b'%');
                    i += 1;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }

    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

// ============================================================================
// Token Exchange
// ============================================================================

/// Exchange an authorization code for tokens.
pub async fn exchange_code(
    config: &ProviderOAuthConfig,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
    client_id: &str,
) -> Result<OAuthTokenSet> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ZeptoError::Config(format!("Failed to create HTTP client: {}", e)))?;

    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", code_verifier),
    ];

    let resp = client
        .post(&config.token_url)
        .form(&params)
        .send()
        .await
        .map_err(|e| ZeptoError::Config(format!("Token exchange request failed: {}", e)))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(ZeptoError::Config(format!(
            "Token exchange failed (HTTP {}): {}",
            status, body
        )));
    }

    let token_resp: TokenResponse = serde_json::from_str(&body).map_err(|e| {
        ZeptoError::Config(format!(
            "Failed to parse token response: {} — body: {}",
            e, body
        ))
    })?;

    let now = chrono::Utc::now().timestamp();
    let expires_at = token_resp.expires_in.map(|secs| now + secs);

    Ok(OAuthTokenSet {
        provider: config.provider.clone(),
        access_token: token_resp.access_token,
        refresh_token: token_resp.refresh_token,
        expires_at,
        token_type: token_resp
            .token_type
            .unwrap_or_else(|| "Bearer".to_string()),
        scope: token_resp.scope,
        obtained_at: now,
        client_id: Some(client_id.to_string()),
    })
}

/// OAuth token endpoint response.
#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    token_type: Option<String>,
    scope: Option<String>,
}

// ============================================================================
// Full OAuth Flow
// ============================================================================

/// Build the authorization URL for the browser.
pub fn build_authorize_url(
    config: &ProviderOAuthConfig,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceChallenge,
    state: &str,
) -> String {
    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}",
        config.authorize_url,
        urlencode(client_id),
        urlencode(redirect_uri),
        urlencode(&pkce.code_challenge),
        urlencode(state),
    );

    if !config.scopes.is_empty() {
        url.push_str("&scope=");
        url.push_str(&urlencode(&config.scopes.join(" ")));
    }

    url
}

/// Minimal URL encoding.
fn urlencode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", b));
            }
        }
    }
    result
}

/// Open a URL in the user's default browser.
pub fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| ZeptoError::Config(format!("Failed to open browser: {}", e)))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| ZeptoError::Config(format!("Failed to open browser: {}", e)))?;
    }

    #[cfg(target_os = "windows")]
    {
        // Use an empty title argument and quote the URL so cmd.exe doesn't treat characters like '&'
        // as command separators.
        let url = url.replace('"', "\\\"");
        let cmd = format!("start \"\" \"{}\"", url);
        std::process::Command::new("cmd")
            .args(["/C", &cmd])
            .spawn()
            .map_err(|e| ZeptoError::Config(format!("Failed to open browser: {}", e)))?;
    }

    Ok(())
}

fn generate_csrf_state() -> String {
    use chacha20poly1305::aead::rand_core::RngCore;
    use chacha20poly1305::aead::OsRng;
    let mut buf = [0u8; 16];
    OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

fn validate_oauth_state(returned_state: Option<&str>, expected_state: &str) -> Result<()> {
    let returned_state = returned_state.ok_or_else(|| {
        ZeptoError::Config("OAuth callback missing state parameter — possible CSRF attack".into())
    })?;

    if returned_state != expected_state {
        return Err(ZeptoError::Config(
            "OAuth state parameter mismatch — possible CSRF attack".into(),
        ));
    }

    Ok(())
}

/// Run the complete OAuth flow for a provider.
///
/// 1. Generate PKCE challenge
/// 2. Start callback server on ephemeral port
/// 3. Open browser to authorization URL
/// 4. Wait for callback
/// 5. Exchange code for tokens
///
/// Returns the obtained token set.
pub async fn run_oauth_flow(
    config: &ProviderOAuthConfig,
    client_id: &str,
) -> Result<OAuthTokenSet> {
    let pkce = PkceChallenge::generate();

    // Generate random state for CSRF protection
    let state = generate_csrf_state();

    // Start callback server first so we know the port
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| ZeptoError::Config(format!("Failed to bind callback server: {}", e)))?;

    let port = listener
        .local_addr()
        .map_err(|e| ZeptoError::Config(format!("Failed to get callback port: {}", e)))?
        .port();

    let redirect_uri = format!("http://127.0.0.1:{}/oauth/callback", port);

    // Build authorization URL
    let auth_url = build_authorize_url(config, client_id, &redirect_uri, &pkce, &state);

    // Open browser
    println!("Opening browser for {} authentication...", config.provider);
    println!();
    println!("If the browser doesn't open, visit this URL manually:");
    println!("  {}", auth_url);
    println!();
    println!("Waiting for authentication (timeout: 120s)...");

    open_browser(&auth_url)?;

    // Wait for callback (120s timeout)
    let callback = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        accept_callback(&listener),
    )
    .await
    .map_err(|_| {
        ZeptoError::Config(
            "OAuth callback timed out after 120s. Did you complete the browser sign-in?".into(),
        )
    })??;

    validate_oauth_state(callback.state.as_deref(), &state)?;

    // Exchange code for tokens
    println!("Exchanging authorization code for tokens...");
    let tokens = exchange_code(
        config,
        &callback.code,
        &pkce.code_verifier,
        &redirect_uri,
        client_id,
    )
    .await?;

    Ok(tokens)
}

/// Run the OAuth flow with a fixed redirect URI port instead of ephemeral.
///
/// Unlike [`run_oauth_flow`], this binds the callback server to a specific port
/// and constructs the redirect URI as `http://localhost:{port}/auth/callback`.
/// This format is required for OpenAI's registered application configuration,
/// which expects `http://localhost:1455/auth/callback` (port 1455, path
/// `/auth/callback`, host `localhost` not `127.0.0.1`).
pub async fn run_oauth_flow_with_port(
    config: &ProviderOAuthConfig,
    client_id: &str,
    port: u16,
) -> Result<OAuthTokenSet> {
    let pkce = PkceChallenge::generate();

    // Generate random state for CSRF protection
    let state = generate_csrf_state();

    // Bind callback server to the fixed port
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .map_err(|e| {
            ZeptoError::Config(format!(
                "Failed to bind callback server on port {}: {} \
                 (is another process using this port?)",
                port, e
            ))
        })?;

    let redirect_uri = format!("http://localhost:{}/auth/callback", port);

    // Build authorization URL
    let auth_url = build_authorize_url(config, client_id, &redirect_uri, &pkce, &state);

    // Open browser
    println!("Opening browser for {} authentication...", config.provider);
    println!();
    println!("If the browser doesn't open, visit this URL manually:");
    println!("  {}", auth_url);
    println!();
    println!("Waiting for authentication (timeout: 120s)...");

    open_browser(&auth_url)?;

    // Wait for callback (120s timeout)
    let callback = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        accept_callback(&listener),
    )
    .await
    .map_err(|_| {
        ZeptoError::Config(
            "OAuth callback timed out after 120s. Did you complete the browser sign-in?".into(),
        )
    })??;

    validate_oauth_state(callback.state.as_deref(), &state)?;

    // Exchange code for tokens
    println!("Exchanging authorization code for tokens...");
    let tokens = exchange_code(
        config,
        &callback.code,
        &pkce.code_verifier,
        &redirect_uri,
        client_id,
    )
    .await?;

    Ok(tokens)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkce_generate() {
        let pkce = PkceChallenge::generate();

        // Code verifier should be base64url-encoded (43 chars for 32 bytes)
        assert!(!pkce.code_verifier.is_empty());
        assert!(pkce.code_verifier.len() >= 43);

        // Code challenge should be base64url-encoded SHA256 (43 chars)
        assert!(!pkce.code_challenge.is_empty());
        assert!(pkce.code_challenge.len() >= 43);

        // Challenge should be SHA256 of verifier
        let mut hasher = Sha256::new();
        hasher.update(pkce.code_verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());
        assert_eq!(pkce.code_challenge, expected);
    }

    #[test]
    fn test_pkce_unique() {
        let a = PkceChallenge::generate();
        let b = PkceChallenge::generate();
        assert_ne!(a.code_verifier, b.code_verifier);
        assert_ne!(a.code_challenge, b.code_challenge);
    }

    #[test]
    fn test_parse_callback_params_success() {
        let result = parse_callback_params("/oauth/callback?code=abc123&state=xyz").unwrap();
        assert_eq!(result.code, "abc123");
        assert_eq!(result.state, Some("xyz".to_string()));
    }

    #[test]
    fn test_parse_callback_params_code_only() {
        let result = parse_callback_params("/oauth/callback?code=abc123").unwrap();
        assert_eq!(result.code, "abc123");
        assert!(result.state.is_none());
    }

    #[test]
    fn test_parse_callback_params_error() {
        let result = parse_callback_params(
            "/oauth/callback?error=access_denied&error_description=User%20denied%20access",
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("access_denied"));
        assert!(err.contains("User denied access"));
    }

    #[test]
    fn test_parse_callback_params_missing_query() {
        let result = parse_callback_params("/oauth/callback");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_callback_params_missing_code() {
        let result = parse_callback_params("/oauth/callback?state=xyz");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_oauth_state_ok() {
        assert!(validate_oauth_state(Some("state123"), "state123").is_ok());
    }

    #[test]
    fn test_validate_oauth_state_missing() {
        let err = validate_oauth_state(None, "state123")
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing state"));
    }

    #[test]
    fn test_validate_oauth_state_mismatch() {
        let err = validate_oauth_state(Some("wrong"), "state123")
            .unwrap_err()
            .to_string();
        assert!(err.contains("state parameter mismatch"));
    }

    #[test]
    fn test_urlencode() {
        assert_eq!(urlencode("hello"), "hello");
        assert_eq!(urlencode("hello world"), "hello%20world");
        assert_eq!(urlencode("a=b&c=d"), "a%3Db%26c%3Dd");
        assert_eq!(urlencode("test-value_123.txt"), "test-value_123.txt");
    }

    #[test]
    fn test_urldecode() {
        assert_eq!(urldecode("hello"), "hello");
        assert_eq!(urldecode("hello%20world"), "hello world");
        assert_eq!(urldecode("hello+world"), "hello world");
        assert_eq!(urldecode("a%3Db"), "a=b");
        assert_eq!(urldecode("%E2%9C%93"), "\u{2713}");
    }

    #[test]
    fn test_build_authorize_url() {
        let config = super::super::ProviderOAuthConfig {
            provider: "test".to_string(),
            token_url: "https://example.com/token".to_string(),
            authorize_url: "https://example.com/authorize".to_string(),
            client_name: "TestApp".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
        };

        let pkce = PkceChallenge {
            code_verifier: "verifier".to_string(),
            code_challenge: "challenge".to_string(),
        };

        let url = build_authorize_url(
            &config,
            "client-id",
            "http://localhost:1234/cb",
            &pkce,
            "state123",
        );

        assert!(url.starts_with("https://example.com/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-id"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state123"));
        assert!(url.contains("scope=read%20write"));
    }

    #[test]
    fn test_build_authorize_url_no_scopes() {
        let config = super::super::ProviderOAuthConfig {
            provider: "test".to_string(),
            token_url: "https://example.com/token".to_string(),
            authorize_url: "https://example.com/authorize".to_string(),
            client_name: "TestApp".to_string(),
            scopes: vec![],
        };

        let pkce = PkceChallenge {
            code_verifier: "v".to_string(),
            code_challenge: "c".to_string(),
        };

        let url = build_authorize_url(&config, "cid", "http://localhost:1234/cb", &pkce, "s");
        assert!(!url.contains("scope="));
    }

    #[test]
    fn test_build_authorize_url_openai_fixed_port() {
        let config = super::super::ProviderOAuthConfig {
            provider: "openai".to_string(),
            token_url: "https://auth.openai.com/oauth/token".to_string(),
            authorize_url: "https://auth.openai.com/oauth/authorize".to_string(),
            client_name: "ZeptoClaw".to_string(),
            scopes: vec!["openid".to_string(), "email".to_string()],
        };
        let pkce = PkceChallenge {
            code_verifier: "v".to_string(),
            code_challenge: "c".to_string(),
        };
        let redirect_uri = "http://localhost:1455/auth/callback";
        let url = build_authorize_url(
            &config,
            "app_EMoamEEZ73f0CkXaXp7hrann",
            redirect_uri,
            &pkce,
            "s",
        );

        assert!(
            url.contains("https://auth.openai.com/oauth/authorize"),
            "URL must start with authorize endpoint"
        );
        assert!(
            url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"),
            "URL must contain client_id"
        );
        assert!(url.contains("1455"), "URL must contain port 1455");
        assert!(url.contains("openid"), "URL must contain openid scope");
    }

    #[tokio::test]
    async fn test_callback_server_timeout() {
        let result = start_callback_server(1).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }
}
