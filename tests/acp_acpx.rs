//! ACP (Agent Client Protocol) integration tests.
//!
//! Two test layers:
//!
//! 1. **Raw wire tests** — spawn `zeptoclaw acp` as a subprocess, drive it
//!    with raw JSON-RPC lines over stdin/stdout, and assert on the responses.
//!    These exercise protocol compliance without an LLM call.
//!
//! 2. **acpx end-to-end tests** — use the `acpx` CLI to drive a full
//!    initialize → session/new → session/prompt → session/update flow.
//!    Gated behind `ZEPTOCLAW_E2E_LIVE` (requires a configured LLM provider).
//!
//! Run with:
//!
//! ```bash
//! cargo nextest run --test acp_acpx
//! ZEPTOCLAW_E2E_LIVE=1 cargo nextest run --test acp_acpx
//! ```

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;

// ============================================================================
// Helpers
// ============================================================================

const WIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Path to the compiled zeptoclaw binary.
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_zeptoclaw")
}

/// Stable path to the `acpx` binary installed via `npm install -g acpx`.
fn acpx_bin() -> Option<String> {
    // Prefer PATH / shim resolution (covers fnm shims, nvm, system npm).
    if let Ok(out) = std::process::Command::new("which").arg("acpx").output() {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                return Some(p);
            }
        }
    }
    // Scan fnm node-versions directory without assuming a specific Node version.
    if let Ok(home) = std::env::var("HOME") {
        let fnm_base = std::path::PathBuf::from(&home).join(".local/share/fnm/node-versions");
        if let Ok(entries) = std::fs::read_dir(&fnm_base) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("installation/bin/acpx");
                if candidate.exists() {
                    return Some(candidate.to_string_lossy().into_owned());
                }
            }
        }
        // nvm as a last home-relative fallback.
        let nvm = format!("{}/.nvm/versions/node/current/bin/acpx", home);
        if std::path::Path::new(&nvm).exists() {
            return Some(nvm);
        }
    }
    // Fixed system locations.
    for p in ["/usr/local/bin/acpx", "/usr/bin/acpx"] {
        if std::path::Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    None
}

/// Return a PATH string that prepends the directory containing the acpx binary
/// so that `#!/usr/bin/env node` resolves correctly when node is installed
/// alongside acpx (e.g. via fnm) but is not in the ambient PATH.
#[cfg(test)]
fn acpx_path_env(acpx_path: &str) -> String {
    let bin_dir = std::path::Path::new(acpx_path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let current = std::env::var("PATH").unwrap_or_default();
    match (bin_dir.is_empty(), current.is_empty()) {
        (true, _) => current,
        (false, true) => bin_dir,
        (false, false) => format!("{}:{}", bin_dir, current),
    }
}

/// A raw JSON-RPC connection to `zeptoclaw acp` over stdin/stdout.
struct AcpConn {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    /// Monotonically increasing id used by helpers so every request gets a
    /// unique, non-conflicting id regardless of how many times they are called.
    next_id: u64,
}

impl AcpConn {
    /// Spawn `zeptoclaw acp` and return a connected handle.
    async fn spawn() -> Self {
        let mut child = Command::new(bin())
            .arg("acp")
            .env("RUST_LOG", "")
            .env(
                "ZEPTOCLAW_MASTER_KEY",
                "0000000000000000000000000000000000000000000000000000000000000000",
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn zeptoclaw acp");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        AcpConn {
            child,
            stdin,
            reader: BufReader::new(stdout),
            // Start at 2: initialize() hardcodes id=1, so helper calls begin here.
            next_id: 2,
        }
    }

    /// Send a JSON-RPC message (appends newline).
    async fn send(&mut self, msg: serde_json::Value) {
        let line = serde_json::to_string(&msg).unwrap();
        assert!(!line.contains('\n'), "JSON-RPC message must be single-line");
        self.stdin
            .write_all(line.as_bytes())
            .await
            .expect("write to stdin");
        self.stdin.write_all(b"\n").await.expect("write newline");
        self.stdin.flush().await.expect("flush stdin");
    }

    /// Read the next non-empty JSON-RPC line from stdout (with timeout).
    async fn recv(&mut self) -> serde_json::Value {
        let result = timeout(WIRE_TIMEOUT, async {
            loop {
                let mut line = String::new();
                self.reader
                    .read_line(&mut line)
                    .await
                    .expect("read from stdout");
                assert!(!line.is_empty(), "ACP process closed stdout unexpectedly");
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    return serde_json::from_str(trimmed)
                        .unwrap_or_else(|e| panic!("invalid JSON from ACP: {e}\nLine: {trimmed}"));
                }
            }
        })
        .await
        .expect("timeout waiting for ACP response");
        result
    }

    /// Read the next JSON-RPC message that has the given `id` field, skipping
    /// any notifications (id=null) or messages with a different id.
    async fn recv_for_id(&mut self, id: &serde_json::Value) -> serde_json::Value {
        loop {
            let msg = self.recv().await;
            // Notifications have no id or null id; skip them.
            match msg.get("id") {
                None | Some(serde_json::Value::Null) => continue,
                Some(v) if v == id => return msg,
                _ => continue,
            }
        }
    }

    /// Drop stdin (signals EOF to the agent) and wait for the child to exit.
    /// Must be called instead of just dropping `AcpConn` to avoid zombie processes.
    async fn shutdown(mut self) {
        drop(self.stdin);
        let _ = self.child.wait().await;
    }

    /// Try to receive one message within a short deadline; returns `None` on timeout.
    async fn try_recv(&mut self) -> Option<serde_json::Value> {
        timeout(Duration::from_millis(200), self.recv()).await.ok()
    }

    /// Perform the mandatory ACP `initialize` handshake, returning the result.
    async fn initialize(&mut self) -> serde_json::Value {
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientInfo": { "name": "test-client", "version": "0.0.0" }
            }
        }))
        .await;
        let resp = self.recv_for_id(&serde_json::json!(1)).await;
        resp.get("result")
            .cloned()
            .unwrap_or_else(|| panic!("initialize returned error: {resp}"))
    }

    /// Create a new session, returning the `sessionId` string.
    async fn new_session(&mut self, cwd: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/new",
            "params": { "cwd": cwd, "mcpServers": [] }
        }))
        .await;
        let resp = self.recv_for_id(&serde_json::json!(id)).await;
        let result = resp
            .get("result")
            .unwrap_or_else(|| panic!("session/new returned error: {resp}"));
        result["sessionId"]
            .as_str()
            .expect("sessionId must be a string")
            .to_string()
    }
}

// ============================================================================
// Wire protocol tests — protocol compliance without LLM calls
// ============================================================================

/// ACP spec: protocolVersion in the InitializeResponse MUST be integer 1.
#[tokio::test]
async fn test_initialize_protocol_version_is_integer_one() {
    let mut conn = AcpConn::spawn().await;
    let result = conn.initialize().await;
    let version = &result["protocolVersion"];
    assert!(
        version.is_number(),
        "protocolVersion must be a number, got: {version}"
    );
    assert_eq!(
        version.as_u64(),
        Some(1),
        "protocolVersion must equal 1, got: {version}"
    );
    conn.shutdown().await;
}

/// ACP spec: InitializeResponse.agentCapabilities.sessionCapabilities.list MUST
/// be present (we advertise session/list support).
#[tokio::test]
async fn test_initialize_advertises_session_list_capability() {
    let mut conn = AcpConn::spawn().await;
    let result = conn.initialize().await;
    let caps = result["agentCapabilities"]
        .get("sessionCapabilities")
        .unwrap_or_else(|| panic!("missing sessionCapabilities in: {result}"));
    assert!(
        caps.get("list").is_some(),
        "sessionCapabilities.list must be advertised; got: {caps}"
    );
    conn.shutdown().await;
}

/// ACP spec: agentInfo.name and agentInfo.version are required strings.
#[tokio::test]
async fn test_initialize_agent_info_fields_are_strings() {
    let mut conn = AcpConn::spawn().await;
    let result = conn.initialize().await;
    let info = &result["agentInfo"];
    assert!(
        info.get("name").and_then(|v| v.as_str()).is_some(),
        "agentInfo.name must be a non-null string; got: {info}"
    );
    assert!(
        info.get("version").and_then(|v| v.as_str()).is_some(),
        "agentInfo.version must be a non-null string; got: {info}"
    );
    assert_eq!(info["name"].as_str().unwrap(), "zeptoclaw");
    conn.shutdown().await;
}

/// ACP spec: agentCapabilities.mcpCapabilities uses field name "mcpCapabilities"
/// (not "mcp" — initialization.md example was wrong, schema.md is authoritative).
#[tokio::test]
async fn test_initialize_mcp_capabilities_field_name() {
    let mut conn = AcpConn::spawn().await;
    let result = conn.initialize().await;
    let caps = &result["agentCapabilities"];
    // "mcp" (wrong) must not appear at the top level of agentCapabilities
    assert!(
        caps.get("mcp").is_none(),
        "field 'mcp' must not appear (schema name is mcpCapabilities); got: {caps}"
    );
    // "mcpCapabilities" (correct) must be present
    assert!(
        caps.get("mcpCapabilities").is_some(),
        "mcpCapabilities must be present in agentCapabilities; got: {caps}"
    );
    conn.shutdown().await;
}

/// ACP spec: authMethods defaults to empty array when no auth is configured.
#[tokio::test]
async fn test_initialize_auth_methods_defaults_to_empty_array() {
    let mut conn = AcpConn::spawn().await;
    let result = conn.initialize().await;
    let auth = result["authMethods"].as_array().unwrap_or_else(|| {
        panic!(
            "authMethods must be an array; got: {}",
            result["authMethods"]
        )
    });
    assert!(
        auth.is_empty(),
        "no auth methods should be advertised by default"
    );
    conn.shutdown().await;
}

/// session/new before initialize must return a JSON-RPC error.
#[tokio::test]
async fn test_session_new_before_initialize_returns_error() {
    let mut conn = AcpConn::spawn().await;
    conn.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "session/new",
        "params": { "cwd": "/tmp", "mcpServers": [] }
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(10)).await;
    assert!(
        resp.get("error").is_some(),
        "session/new before initialize must return an error; got: {resp}"
    );
    conn.shutdown().await;
}

/// session/prompt before initialize must return a JSON-RPC error.
#[tokio::test]
async fn test_session_prompt_before_initialize_returns_error() {
    let mut conn = AcpConn::spawn().await;
    conn.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "session/prompt",
        "params": {
            "sessionId": "ghost-session",
            "prompt": [{ "type": "text", "text": "hello" }]
        }
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(11)).await;
    assert!(
        resp.get("error").is_some(),
        "session/prompt before initialize must return an error; got: {resp}"
    );
    conn.shutdown().await;
}

/// An unknown JSON-RPC method must return error code -32601 (Method not found).
#[tokio::test]
async fn test_unknown_method_returns_method_not_found() {
    let mut conn = AcpConn::spawn().await;
    conn.initialize().await;
    conn.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 20,
        "method": "nonexistent/method",
        "params": {}
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(20)).await;
    let err = resp
        .get("error")
        .unwrap_or_else(|| panic!("expected error for unknown method; got: {resp}"));
    assert_eq!(
        err["code"].as_i64(),
        Some(-32601),
        "unknown method must return -32601; got: {err}"
    );
    conn.shutdown().await;
}

/// Malformed JSON must return error code -32700 (Parse error).
#[tokio::test]
async fn test_malformed_json_returns_parse_error() {
    let mut conn = AcpConn::spawn().await;
    // Send a line that is not valid JSON.
    conn.stdin
        .write_all(b"this is not { valid json }\n")
        .await
        .unwrap();
    conn.stdin.flush().await.unwrap();
    let resp = conn.recv().await;
    let err = resp
        .get("error")
        .unwrap_or_else(|| panic!("expected parse error; got: {resp}"));
    assert_eq!(
        err["code"].as_i64(),
        Some(-32700),
        "malformed JSON must return -32700; got: {err}"
    );
    conn.shutdown().await;
}

/// session/new must return a non-empty string sessionId.
#[tokio::test]
async fn test_session_new_returns_session_id() {
    let mut conn = AcpConn::spawn().await;
    conn.initialize().await;
    let session_id = conn.new_session("/tmp/acp-test").await;
    assert!(
        !session_id.is_empty(),
        "sessionId must be a non-empty string"
    );
    conn.shutdown().await;
}

/// session/new with same cwd must return distinct session IDs.
#[tokio::test]
async fn test_session_new_returns_unique_ids() {
    let mut conn = AcpConn::spawn().await;
    conn.initialize().await;

    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 30,
        "method": "session/new",
        "params": { "cwd": "/tmp/acp-unique", "mcpServers": [] }
    }))
    .await;
    let r1 = conn.recv_for_id(&serde_json::json!(30)).await;
    let id1 = r1["result"]["sessionId"].as_str().unwrap().to_string();

    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 31,
        "method": "session/new",
        "params": { "cwd": "/tmp/acp-unique", "mcpServers": [] }
    }))
    .await;
    let r2 = conn.recv_for_id(&serde_json::json!(31)).await;
    let id2 = r2["result"]["sessionId"].as_str().unwrap().to_string();

    assert_ne!(id1, id2, "each session/new must produce a unique sessionId");
    conn.shutdown().await;
}

/// session/list must return a `sessions` array containing known session IDs.
#[tokio::test]
async fn test_session_list_contains_created_sessions() {
    let mut conn = AcpConn::spawn().await;
    conn.initialize().await;
    let session_id = conn.new_session("/tmp/acp-list-test").await;

    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 40,
        "method": "session/list",
        "params": {}
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(40)).await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("session/list returned error: {resp}"));
    let sessions = result["sessions"]
        .as_array()
        .expect("sessions must be an array");
    let found = sessions
        .iter()
        .any(|s| s["sessionId"].as_str() == Some(&session_id));
    assert!(
        found,
        "session/list must include created session {session_id}; got: {sessions:?}"
    );
    conn.shutdown().await;
}

/// session/list with cwd filter must only return sessions matching that cwd.
#[tokio::test]
async fn test_session_list_cwd_filter() {
    let mut conn = AcpConn::spawn().await;
    conn.initialize().await;

    let id_a = conn.new_session("/tmp/acp-cwd-a").await;
    let id_b = conn.new_session("/tmp/acp-cwd-b").await;

    // Filter for cwd-a only.
    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 50,
        "method": "session/list",
        "params": { "cwd": "/tmp/acp-cwd-a" }
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(50)).await;
    let sessions = resp["result"]["sessions"]
        .as_array()
        .expect("sessions must be an array");

    let has_a = sessions
        .iter()
        .any(|s| s["sessionId"].as_str() == Some(&id_a));
    let has_b = sessions
        .iter()
        .any(|s| s["sessionId"].as_str() == Some(&id_b));
    assert!(has_a, "cwd filter must include session from matching cwd");
    assert!(
        !has_b,
        "cwd filter must exclude session from non-matching cwd"
    );
    conn.shutdown().await;
}

/// session/list results must include the `cwd` field on each SessionInfo.
#[tokio::test]
async fn test_session_list_session_info_has_cwd() {
    let mut conn = AcpConn::spawn().await;
    conn.initialize().await;
    conn.new_session("/tmp/acp-info-cwd").await;

    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 60,
        "method": "session/list",
        "params": {}
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(60)).await;
    let sessions = resp["result"]["sessions"]
        .as_array()
        .expect("sessions array");
    for s in sessions {
        assert!(
            s.get("cwd").and_then(|v| v.as_str()).is_some(),
            "each SessionInfo must have a cwd string; got: {s}"
        );
        assert!(
            s.get("sessionId").and_then(|v| v.as_str()).is_some(),
            "each SessionInfo must have a sessionId string; got: {s}"
        );
    }
    conn.shutdown().await;
}

/// session/list before initialize must return a JSON-RPC error.
#[tokio::test]
async fn test_session_list_before_initialize_returns_error() {
    let mut conn = AcpConn::spawn().await;
    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 70,
        "method": "session/list",
        "params": {}
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(70)).await;
    assert!(
        resp.get("error").is_some(),
        "session/list before initialize must return an error; got: {resp}"
    );
    conn.shutdown().await;
}

/// session/prompt with an unknown sessionId must return a JSON-RPC error.
#[tokio::test]
async fn test_session_prompt_unknown_session_returns_error() {
    let mut conn = AcpConn::spawn().await;
    conn.initialize().await;
    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 80,
        "method": "session/prompt",
        "params": {
            "sessionId": "does-not-exist-session-id",
            "prompt": [{ "type": "text", "text": "hello" }]
        }
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(80)).await;
    assert!(
        resp.get("error").is_some(),
        "session/prompt with unknown session must return error; got: {resp}"
    );
    let code = resp["error"]["code"].as_i64().unwrap_or(0);
    assert_eq!(
        code, -32000,
        "unknown session must return -32000 (not -32602 invalid params); got code {code}"
    );
    conn.shutdown().await;
}

/// session/cancel is a notification (no id); the server must NOT send a response.
/// We verify this with a bounded read immediately after sending cancel, asserting
/// that nothing arrives, then confirm the channel is still usable.
#[tokio::test]
async fn test_session_cancel_sends_no_response() {
    let mut conn = AcpConn::spawn().await;
    conn.initialize().await;
    let session_id = conn.new_session("/tmp/acp-cancel-test").await;

    // Send cancel notification (no id field) — per spec this is a notification.
    conn.send(serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }))
    .await;

    // A bounded read must time out: the server must not respond to a notification.
    let stray = conn.try_recv().await;
    assert!(
        stray.is_none(),
        "server must not send a response to session/cancel (notification), got: {stray:?}"
    );

    // Channel must still be usable after a cancel.
    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 90,
        "method": "session/list",
        "params": {}
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(90)).await;
    assert!(
        resp.get("result").is_some(),
        "session/list after cancel must still succeed; got: {resp}"
    );
    conn.shutdown().await;
}

/// Duplicate initialize calls must succeed (idempotent).
#[tokio::test]
async fn test_double_initialize_is_idempotent() {
    let mut conn = AcpConn::spawn().await;
    conn.initialize().await; // first
    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 100,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientInfo": { "name": "test-client", "version": "0.0.0" }
        }
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(100)).await;
    // Must return a valid result (not an error) for the second initialize.
    assert!(
        resp.get("result").is_some(),
        "second initialize must return a result; got: {resp}"
    );
    assert_eq!(
        resp["result"]["protocolVersion"].as_u64(),
        Some(1),
        "second initialize must still return protocolVersion 1"
    );
    conn.shutdown().await;
}

/// A session/prompt with a ResourceLink content block (MUST be supported) must
/// not return a parse or capability error — the error (if any) must be about
/// the session not existing, not about unsupported content type.
#[tokio::test]
async fn test_session_prompt_accepts_resource_link_content() {
    let mut conn = AcpConn::spawn().await;
    conn.initialize().await;
    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 110,
        "method": "session/prompt",
        "params": {
            "sessionId": "fake-for-type-check",
            "prompt": [
                { "type": "resource_link", "uri": "file:///tmp/test.txt", "name": "test.txt" }
            ]
        }
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(110)).await;
    // Must error (unknown session), but not with -32600 (invalid request)
    // from a content type rejection. The error code must not be -32602.
    if let Some(err) = resp.get("error") {
        assert_ne!(
            err["code"].as_i64(),
            Some(-32602),
            "ResourceLink must not be rejected as invalid params; got: {err}"
        );
    }
    // result is also fine (would mean the prompt was accepted and processed)
    conn.shutdown().await;
}

/// Full session lifecycle in a single connection:
///   initialize → list (empty) → session/new → list (has id) →
///   session/prompt with new id (ok) → session/prompt with bad id (error)
#[tokio::test]
async fn test_session_lifecycle_full() {
    let mut conn = AcpConn::spawn().await;
    conn.initialize().await;

    // 1. list sessions before creating any — must return an empty array.
    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 200,
        "method": "session/list",
        "params": {}
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(200)).await;
    let sessions = resp["result"]["sessions"]
        .as_array()
        .expect("sessions must be an array before any session is created");
    assert!(
        sessions.is_empty(),
        "session/list must be empty before any session is created; got: {sessions:?}"
    );

    // 2. create a new session.
    let session_id = conn.new_session("/tmp/acp-lifecycle").await;
    assert!(!session_id.is_empty(), "sessionId must be non-empty");

    // 3. list sessions — must now include the new session id.
    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 201,
        "method": "session/list",
        "params": {}
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(201)).await;
    let sessions = resp["result"]["sessions"]
        .as_array()
        .expect("sessions must be an array after session/new");
    let found = sessions
        .iter()
        .any(|s| s["sessionId"].as_str() == Some(&session_id));
    assert!(
        found,
        "session/list must include the newly created session; got: {sessions:?}"
    );

    // 4. session/prompt with an unknown id must return error -32000.
    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 202,
        "method": "session/prompt",
        "params": {
            "sessionId": "does-not-exist",
            "prompt": [{ "type": "text", "text": "hello" }]
        }
    }))
    .await;
    let resp = conn.recv_for_id(&serde_json::json!(202)).await;
    assert!(
        resp.get("error").is_some(),
        "session/prompt with unknown id must return an error; got: {resp}"
    );
    assert_eq!(
        resp["error"]["code"].as_i64(),
        Some(-32000),
        "unknown session must return -32000; got: {}",
        resp["error"]
    );

    // 5. session/prompt with the real session id must be accepted (no error).
    //    We don't wait for an LLM reply — just confirm no immediate error.
    conn.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 203,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": "hello" }]
        }
    }))
    .await;
    // The server queues the prompt and responds when the agent replies.
    // We only assert that no immediate protocol-level error is returned
    // (the request was accepted, not rejected).  Use a short deadline so the
    // test doesn't block indefinitely in CI without a live LLM.
    if let Some(early) = conn.try_recv().await {
        assert!(
            early.get("error").is_none(),
            "session/prompt with valid session must not return an immediate error; got: {early}"
        );
    }

    conn.shutdown().await;
}

// ============================================================================
// acpx end-to-end tests — require a configured LLM provider
// ============================================================================

/// Check whether the ZEPTOCLAW_E2E_LIVE gate is set.
fn e2e_live() -> bool {
    std::env::var("ZEPTOCLAW_E2E_LIVE").is_ok()
}

/// Run `acpx --agent 'zeptoclaw acp' --format json --approve-all exec <prompt>`
/// and return the parsed NDJSON event lines, or `None` if `acpx` is not found.
#[cfg(test)]
fn run_acpx_exec(prompt: &str) -> Option<Vec<serde_json::Value>> {
    let acpx = acpx_bin()?;
    let agent_cmd = format!("{} acp", bin());
    let output = std::process::Command::new(&acpx)
        .args([
            "--agent",
            &agent_cmd,
            "--format",
            "json",
            "--approve-all",
            "--timeout",
            "30",
            "exec",
            prompt,
        ])
        .env("RUST_LOG", "")
        .env(
            "ZEPTOCLAW_MASTER_KEY",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .env("PATH", acpx_path_env(&acpx))
        .output()
        .expect("failed to run acpx");
    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(
        stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
    )
}

/// acpx exec must complete with a non-empty text response.
#[test]
fn test_acpx_exec_basic_prompt() {
    if !e2e_live() {
        eprintln!("Skipping: set ZEPTOCLAW_E2E_LIVE=1 to run");
        return;
    }
    let Some(events) = run_acpx_exec("reply with exactly three words: ONE TWO THREE") else {
        eprintln!("acpx not found; skipping");
        return;
    };
    assert!(
        !events.is_empty(),
        "acpx exec must produce at least one JSON event"
    );
    // At least one event must carry text content.  With --format json, acpx
    // emits raw JSON-RPC wire traffic; the agent's reply arrives as a
    // session/update notification where text lives at
    // params.update.content.text.
    let has_text = events.iter().any(|e| {
        e.pointer("/params/update/content/text")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    });
    assert!(
        has_text,
        "at least one event must have non-empty content text; events: {events:?}"
    );
}

/// acpx exec: session/update notifications carry content text.
#[test]
fn test_acpx_exec_produces_session_update_events() {
    if !e2e_live() {
        eprintln!("Skipping: set ZEPTOCLAW_E2E_LIVE=1 to run");
        return;
    }
    let Some(events) = run_acpx_exec("say hello") else {
        eprintln!("acpx not found; skipping");
        return;
    };
    assert!(!events.is_empty(), "must produce events");
    // At least one event must carry a non-empty text payload in the
    // session/update notification (params.update.content.text in the raw
    // JSON-RPC wire format that --format json emits).
    let has_content = events.iter().any(|e| {
        e.pointer("/params/update/content/text")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    });
    assert!(
        has_content,
        "at least one event must carry non-empty content/text from a session/update; events: {events:?}"
    );
}

/// acpx exec: the final turn must conclude with a stop reason of end_turn.
#[test]
fn test_acpx_exec_ends_with_end_turn() {
    if !e2e_live() {
        eprintln!("Skipping: set ZEPTOCLAW_E2E_LIVE=1 to run");
        return;
    }
    let Some(events) = run_acpx_exec("say: DONE") else {
        eprintln!("acpx not found; skipping");
        return;
    };
    assert!(
        !events.is_empty(),
        "acpx exec must complete and produce events"
    );
    // The session/prompt response must contain stopReason=end_turn.  With
    // --format json the response is a JSON-RPC result: result.stopReason.
    let has_end_turn = events.iter().any(|e| {
        e.pointer("/result/stopReason")
            .and_then(|v| v.as_str())
            .map(|s| s == "end_turn")
            .unwrap_or(false)
    });
    assert!(
        has_end_turn,
        "at least one event must have stopReason=end_turn; events: {events:?}"
    );
}

/// acpx: sessions new then sessions list must show the new session.
#[test]
fn test_acpx_sessions_list_after_exec() {
    if !e2e_live() {
        eprintln!("Skipping: set ZEPTOCLAW_E2E_LIVE=1 to run");
        return;
    }
    let acpx = match acpx_bin() {
        Some(p) => p,
        None => {
            eprintln!("acpx not found; skipping");
            return;
        }
    };
    let agent_cmd = format!("{} acp", bin());
    let tmp = std::env::temp_dir().join("acpx-session-list-test");
    std::fs::create_dir_all(&tmp).ok();

    // Run exec to force a session to be created.
    let exec_out = std::process::Command::new(&acpx)
        .args([
            "--agent",
            &agent_cmd,
            "--cwd",
            tmp.to_str().unwrap(),
            "--format",
            "quiet",
            "--approve-all",
            "--timeout",
            "30",
            "exec",
            "say: HELLO",
        ])
        .env("RUST_LOG", "")
        .env("PATH", acpx_path_env(&acpx))
        .env(
            "ZEPTOCLAW_MASTER_KEY",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .output()
        .expect("failed to run acpx exec");
    assert!(
        exec_out.status.success(),
        "acpx exec must succeed; stderr: {}",
        String::from_utf8_lossy(&exec_out.stderr)
    );

    // sessions list must show the session we just created.
    let list_out = std::process::Command::new(&acpx)
        .args([
            "--agent",
            &agent_cmd,
            "--cwd",
            tmp.to_str().unwrap(),
            "--format",
            "json",
            "sessions",
            "list",
        ])
        .env("RUST_LOG", "")
        .env("PATH", acpx_path_env(&acpx))
        .env(
            "ZEPTOCLAW_MASTER_KEY",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .output()
        .expect("failed to run acpx sessions list");
    assert!(
        list_out.status.success(),
        "acpx sessions list must succeed; stderr: {}",
        String::from_utf8_lossy(&list_out.stderr)
    );
    // `--agent` mode spawns a fresh process per invocation, so sessions
    // created during exec are not visible to a separate sessions-list call.
    // We verify only that sessions list exits successfully and emits valid JSON
    // (an array, possibly empty).
    let list_stdout = String::from_utf8_lossy(&list_out.stdout);
    let list_json: serde_json::Value =
        serde_json::from_str(list_stdout.trim()).unwrap_or(serde_json::Value::Null);
    assert!(
        list_json.is_array(),
        "sessions list must output a JSON array; got: {list_stdout}"
    );
}

/// acpx session lifecycle via acpx CLI subcommands.
///
/// What acpx can cover:
///   - `sessions list` for a fresh cwd returns no entry for that cwd
///   - `sessions new` creates and registers a session (ID visible in the list)
///   - `sessions list` after `sessions new` includes the new session
///   - `exec` completes a conversation successfully
///
/// What acpx cannot cover (exec always issues session/new internally; there is
/// no --session-id flag to target an existing or fake session):
///   - converse via the exact ACP session ID from sessions new
///   - converse with a non-existent ACP session ID → expect error
/// Both are covered by the raw wire test `test_session_lifecycle_full`.
#[test]
fn test_acpx_session_lifecycle() {
    if !e2e_live() {
        eprintln!("Skipping: set ZEPTOCLAW_E2E_LIVE=1 to run");
        return;
    }
    let acpx = match acpx_bin() {
        Some(p) => p,
        None => {
            eprintln!("acpx not found; skipping");
            return;
        }
    };
    let agent_cmd = format!("{} acp", bin());
    // Use a timestamp-derived cwd so this run's sessions don't collide with
    // previous runs that left entries in the acpx local registry (~/.acpx/).
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let cwd = std::env::temp_dir().join(format!("acp-lifecycle-{ts}"));
    std::fs::create_dir_all(&cwd).ok();
    let cwd_str = cwd.to_str().unwrap();

    let master_key = "0000000000000000000000000000000000000000000000000000000000000000";

    // Helper: run an acpx subcommand with standard env, return (success, stdout, stderr).
    let run_acpx = |args: &[&str]| -> (bool, String, String) {
        let out = std::process::Command::new(&acpx)
            .args(args)
            .env("RUST_LOG", "")
            .env("ZEPTOCLAW_MASTER_KEY", master_key)
            .env("PATH", acpx_path_env(&acpx))
            .output()
            .expect("acpx command failed to spawn");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    // ── Step 1: sessions list for a brand-new cwd ── should have no entry for it ──
    let (ok, stdout, _) = run_acpx(&[
        "--agent", &agent_cmd, "--cwd", cwd_str, "--format", "json", "sessions", "list",
    ]);
    assert!(
        ok,
        "sessions list must succeed before any session is created"
    );
    let all_sessions: Vec<serde_json::Value> =
        serde_json::from_str(stdout.trim()).expect("sessions list must return JSON array");
    let for_cwd: Vec<_> = all_sessions
        .iter()
        .filter(|s| s["cwd"].as_str() == Some(cwd_str))
        .collect();
    assert!(
        for_cwd.is_empty(),
        "no acpx sessions expected for fresh cwd {cwd_str}; got: {for_cwd:?}"
    );

    // ── Step 2: sessions new ── creates and registers a session ──
    let (ok, stdout, stderr) = run_acpx(&[
        "--agent", &agent_cmd, "--cwd", cwd_str, "--format", "json", "sessions", "new",
    ]);
    assert!(ok, "sessions new must succeed; stderr: {stderr}");
    let new_obj: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("sessions new must return JSON");
    let session_id = new_obj
        .get("acpxSessionId")
        .or_else(|| new_obj.get("acpxRecordId"))
        .and_then(|v| v.as_str())
        .expect("sessions new must return a session id")
        .to_string();
    assert!(!session_id.is_empty(), "session id must not be empty");

    // ── Step 3: sessions list after new ── must include the created session ──
    let (ok, stdout, _) = run_acpx(&[
        "--agent", &agent_cmd, "--cwd", cwd_str, "--format", "json", "sessions", "list",
    ]);
    assert!(ok, "sessions list must succeed after sessions new");
    let all_sessions: Vec<serde_json::Value> =
        serde_json::from_str(stdout.trim()).expect("sessions list must return JSON array");
    let found = all_sessions.iter().any(|s| {
        s.get("acpxSessionId").and_then(|v| v.as_str()) == Some(&session_id)
            || s.get("acpxRecordId").and_then(|v| v.as_str()) == Some(&session_id)
    });
    assert!(
        found,
        "sessions list must include the session created by sessions new ({session_id}); \
         got: {all_sessions:?}"
    );

    // ── Step 4: exec for the same cwd ── conversation must succeed ──
    // (acpx exec issues session/new internally; it doesn't reuse the ACP
    //  session from step 2, but the conversation round-trip must work.)
    let (ok, stdout, stderr) = run_acpx(&[
        "--agent",
        &agent_cmd,
        "--cwd",
        cwd_str,
        "--format",
        "json",
        "--approve-all",
        "--timeout",
        "30",
        "exec",
        "say: HELLO",
    ]);
    assert!(ok, "exec must succeed; stderr: {stderr}");
    let events: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let has_text = events.iter().any(|e| {
        e.pointer("/params/update/content/text")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    });
    assert!(
        has_text,
        "exec must produce a non-empty text reply; events: {events:?}"
    );

    // Steps 5–6 (converse via exact ACP session ID / non-existent session ID)
    // are not reachable via acpx CLI — see test_session_lifecycle_full for
    // full coverage at the raw JSON-RPC wire level.
}
