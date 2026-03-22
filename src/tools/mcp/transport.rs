//! MCP transport abstractions — HTTP and stdio.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use super::protocol::{McpRequest, McpResponse};

/// Transport layer for MCP JSON-RPC communication.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and return the response.
    async fn send(&self, request: &McpRequest) -> Result<McpResponse, String>;

    /// Gracefully shut down the transport (kill child process, close connection, etc.).
    async fn shutdown(&self) -> Result<(), String>;

    /// Returns the transport type identifier ("http" or "stdio").
    fn transport_type(&self) -> &str;
}

/// HTTP transport for MCP — sends JSON-RPC requests via POST.
pub struct HttpTransport {
    url: String,
    http: reqwest::Client,
}

impl HttpTransport {
    /// Create a new HTTP transport.
    pub fn new(url: &str, timeout_secs: u64) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .unwrap_or_default();
        Self {
            url: url.to_string(),
            http,
        }
    }

    /// Get the server URL.
    pub fn url(&self) -> &str {
        &self.url
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send(&self, request: &McpRequest) -> Result<McpResponse, String> {
        let resp = self
            .http
            .post(&self.url)
            .json(request)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {} from MCP server: {}", status, body));
        }

        resp.json::<McpResponse>()
            .await
            .map_err(|e| format!("Failed to parse MCP response: {}", e))
    }

    async fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }

    fn transport_type(&self) -> &str {
        "http"
    }
}

/// Stdio transport for MCP — spawns a child process and communicates via
/// header-framed JSON-RPC over stdin/stdout (Content-Length framing per
/// the MCP stdio specification).
///
/// Stdin and stdout are guarded by a single mutex to prevent request/response
/// interleaving when multiple tool calls execute concurrently.
pub struct StdioTransport {
    /// Combined stdin+stdout lock — serializes the entire send/receive cycle
    /// so concurrent callers cannot interleave requests and misroute responses.
    io: Arc<Mutex<StdioIo>>,
    child: Arc<Mutex<Child>>,
    timeout_secs: u64,
}

/// Bundled stdin/stdout handles protected by a single lock.
struct StdioIo {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl StdioTransport {
    /// Spawn a child process and return a StdioTransport connected to it.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        timeout_secs: u64,
    ) -> Result<Self, String> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn MCP server '{}': {}", command, e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to capture child stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture child stdout".to_string())?;

        Ok(Self {
            io: Arc::new(Mutex::new(StdioIo {
                stdin,
                stdout: BufReader::new(stdout),
            })),
            child: Arc::new(Mutex::new(child)),
            timeout_secs,
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, request: &McpRequest) -> Result<McpResponse, String> {
        let body = serde_json::to_string(request)
            .map_err(|e| format!("Failed to serialize request: {}", e))?;

        // MCP stdio framing: Content-Length header + \r\n separator + JSON body.
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        let timeout = std::time::Duration::from_secs(self.timeout_secs);

        // Hold a single lock for the entire write→read cycle to prevent
        // concurrent callers from interleaving requests and misrouting responses.
        let mut io = self.io.lock().await;

        // --- Write framed request ---
        tokio::time::timeout(timeout, io.stdin.write_all(frame.as_bytes()))
            .await
            .map_err(|_| "Timeout writing to MCP server stdin".to_string())?
            .map_err(|e| format!("Failed to write to MCP server stdin: {}", e))?;
        tokio::time::timeout(timeout, io.stdin.flush())
            .await
            .map_err(|_| "Timeout flushing MCP server stdin".to_string())?
            .map_err(|e| format!("Failed to flush MCP server stdin: {}", e))?;

        // --- Read framed response (Content-Length header + body) ---
        let content_length = tokio::time::timeout(timeout, read_content_length(&mut io.stdout))
            .await
            .map_err(|_| "Timeout reading Content-Length from MCP server".to_string())?
            .map_err(|e| format!("Failed to read Content-Length: {}", e))?;

        let mut buf = vec![0u8; content_length];
        tokio::time::timeout(timeout, io.stdout.read_exact(&mut buf))
            .await
            .map_err(|_| "Timeout reading response body from MCP server".to_string())?
            .map_err(|e| format!("Failed to read response body: {}", e))?;

        serde_json::from_slice::<McpResponse>(&buf)
            .map_err(|e| format!("Failed to parse MCP stdio response: {}", e))
    }

    async fn shutdown(&self) -> Result<(), String> {
        let mut child = self.child.lock().await;

        match tokio::time::timeout(std::time::Duration::from_secs(3), child.wait()).await {
            Ok(_) => Ok(()),
            Err(_) => {
                child
                    .kill()
                    .await
                    .map_err(|e| format!("Failed to kill MCP server: {}", e))?;
                Ok(())
            }
        }
    }

    fn transport_type(&self) -> &str {
        "stdio"
    }
}

/// Read headers from a MCP stdio stream until an empty line, returning the
/// `Content-Length` value.  Headers follow the pattern `Key: Value\r\n` with
/// the header block terminated by a bare `\r\n`.
async fn read_content_length<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<usize, String> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut header_line = String::new();
        let n = reader
            .read_line(&mut header_line)
            .await
            .map_err(|e| format!("Failed to read header line: {}", e))?;
        if n == 0 {
            return Err("MCP server closed stdout while reading headers".to_string());
        }
        let trimmed = header_line.trim();
        if trimmed.is_empty() {
            // End of headers.
            break;
        }
        if let Some(val) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(
                val.trim()
                    .parse::<usize>()
                    .map_err(|e| format!("Invalid Content-Length value: {}", e))?,
            );
        }
        // Ignore other headers (per spec, only Content-Length is required).
    }
    content_length.ok_or_else(|| "MCP server response missing Content-Length header".to_string())
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::mcp::protocol::McpRequest;

    #[test]
    fn test_http_transport_type() {
        let t = HttpTransport::new("http://localhost:8080", 30);
        assert_eq!(t.transport_type(), "http");
    }

    #[test]
    fn test_http_transport_url() {
        let t = HttpTransport::new("http://localhost:8080", 30);
        assert_eq!(t.url(), "http://localhost:8080");
    }

    #[tokio::test]
    async fn test_http_transport_send_no_server() {
        let t = HttpTransport::new("http://127.0.0.1:1", 5);
        let req = McpRequest::new(1, "tools/list", None);
        let result = t.send(&req).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("HTTP request failed"));
    }

    #[tokio::test]
    async fn test_http_transport_shutdown_is_noop() {
        let t = HttpTransport::new("http://localhost:8080", 30);
        let result = t.shutdown().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_read_content_length_valid() {
        let input = b"Content-Length: 42\r\n\r\n";
        let mut reader = BufReader::new(&input[..]);
        let len = read_content_length(&mut reader).await.unwrap();
        assert_eq!(len, 42);
    }

    #[tokio::test]
    async fn test_read_content_length_missing() {
        let input = b"X-Custom: foo\r\n\r\n";
        let mut reader = BufReader::new(&input[..]);
        let result = read_content_length(&mut reader).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing Content-Length"));
    }

    #[tokio::test]
    async fn test_stdio_transport_content_length_framing() {
        // Spawn a bash script that reads Content-Length framed input and
        // echoes it back with Content-Length framing (minimal MCP echo).
        let script = r#"
while IFS= read -r line; do
  line="${line%%$'\r'}"
  if [[ "$line" == Content-Length:* ]]; then
    cl="${line#Content-Length: }"
  fi
  if [[ -z "$line" ]]; then
    body=$(dd bs=1 count="$cl" 2>/dev/null)
    printf "Content-Length: %d\r\n\r\n%s" "${#body}" "$body"
  fi
done
"#;
        let transport = StdioTransport::spawn(
            "bash",
            &["-c".to_string(), script.to_string()],
            &HashMap::new(),
            10,
        )
        .await;
        assert!(
            transport.is_ok(),
            "bash echo should spawn: {:?}",
            transport.err()
        );
        let t = transport.unwrap();
        assert_eq!(t.transport_type(), "stdio");

        let req = McpRequest::new(1, "initialize", None);
        let resp = t.send(&req).await;
        // cat-style echo: response parses as valid JSON-RPC (same structure as request).
        assert!(
            resp.is_ok(),
            "Content-Length framing roundtrip should succeed: {:?}",
            resp.err()
        );

        let _ = t.shutdown().await;
    }

    #[tokio::test]
    async fn test_stdio_transport_spawn_nonexistent_command() {
        let result = StdioTransport::spawn(
            "/nonexistent/binary/that/does/not/exist",
            &[],
            &HashMap::new(),
            5,
        )
        .await;
        assert!(result.is_err(), "Spawning nonexistent binary should fail");
    }

    #[tokio::test]
    async fn test_stdio_transport_shutdown_kills_process() {
        let transport = StdioTransport::spawn("cat", &[], &HashMap::new(), 10)
            .await
            .unwrap();

        let result = transport.shutdown().await;
        assert!(result.is_ok(), "Shutdown should succeed");
    }
}
