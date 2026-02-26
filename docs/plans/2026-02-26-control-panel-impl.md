# ZeptoClaw Control Panel — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add an axum-based API server and React dashboard to ZeptoClaw as a monorepo workspace, started via `zeptoclaw panel`.

**Architecture:** axum serves REST + WebSocket on `:9091` and static files from `panel/dist/` on `:9092`. A `tokio::broadcast` event bus bridges agent loop events to WebSocket clients. The React frontend (Vite + Tailwind) consumes both REST and WebSocket. New `TaskTool` gives the agent kanban board access.

**Tech Stack:** Rust (axum, tower-http, jsonwebtoken, bcrypt), React 19, Vite 6, Tailwind CSS 4, @tanstack/react-query, @dnd-kit, recharts, react-router

**Design doc:** `docs/plans/2026-02-26-control-panel-design.md`

---

## Phase 1: API Server Foundation

### Task 1: Add axum + tower-http dependencies

**Files:**
- Modify: `Cargo.toml`

**Step 1: Add dependencies to Cargo.toml**

Add under `[dependencies]`:
```toml
# =============================================================================
# API SERVER (panel)
# =============================================================================
axum = { version = "0.8", features = ["ws"] }
tower-http = { version = "0.6", features = ["cors", "fs", "trace"] }
jsonwebtoken = "9"
bcrypt = "0.17"
```

**Step 2: Verify build**

Run: `cargo check`
Expected: compiles with new deps

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add axum, tower-http, jwt, bcrypt dependencies for panel"
```

---

### Task 2: Create PanelConfig

**Files:**
- Create: `src/api/config.rs`
- Modify: `src/config/types.rs` (add `pub panel: ...` field to `Config`)
- Modify: `src/config/mod.rs` (add env overrides)
- Modify: `src/config/validate.rs` (add `"panel"` to `KNOWN_TOP_LEVEL`)

**Step 1: Write tests for PanelConfig defaults**

In `src/api/config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panel_config_defaults() {
        let cfg = PanelConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.port, 9092);
        assert_eq!(cfg.api_port, 9091);
        assert_eq!(cfg.auth_mode, AuthMode::Token);
        assert_eq!(cfg.bind, "127.0.0.1");
    }

    #[test]
    fn test_auth_mode_serde() {
        let json = r#""password""#;
        let mode: AuthMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode, AuthMode::Password);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib api::config`
Expected: FAIL — module doesn't exist

**Step 3: Implement PanelConfig**

```rust
//! Panel configuration types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    Token,
    Password,
    None,
}

impl Default for AuthMode {
    fn default() -> Self { Self::Token }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelConfig {
    pub enabled: bool,
    pub port: u16,
    pub api_port: u16,
    pub auth_mode: AuthMode,
    pub bind: String,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 9092,
            api_port: 9091,
            auth_mode: AuthMode::Token,
            bind: "127.0.0.1".to_string(),
        }
    }
}
```

**Step 4: Wire into Config struct**

In `src/config/types.rs`, add after `pub logging: LoggingConfig`:
```rust
/// Panel (control panel) configuration.
#[serde(default)]
pub panel: crate::api::config::PanelConfig,
```

In `src/config/mod.rs` `apply_env_overrides()`, add:
```rust
if let Ok(val) = std::env::var("ZEPTOCLAW_PANEL_ENABLED") {
    self.panel.enabled = val.to_lowercase() == "true";
}
if let Ok(val) = std::env::var("ZEPTOCLAW_PANEL_PORT") {
    if let Ok(v) = val.parse() { self.panel.port = v; }
}
if let Ok(val) = std::env::var("ZEPTOCLAW_PANEL_API_PORT") {
    if let Ok(v) = val.parse() { self.panel.api_port = v; }
}
if let Ok(val) = std::env::var("ZEPTOCLAW_PANEL_BIND") {
    self.panel.bind = val;
}
```

In `src/config/validate.rs`, add `"panel"` to `KNOWN_TOP_LEVEL` array.

**Step 5: Create `src/api/mod.rs`**

```rust
pub mod config;
```

**Step 6: Add to `src/lib.rs`**

```rust
pub mod api;
```

**Step 7: Run tests**

Run: `cargo test --lib api::config`
Expected: PASS

**Step 8: Commit**

```bash
git add src/api/ src/config/types.rs src/config/mod.rs src/config/validate.rs src/lib.rs
git commit -m "feat(panel): add PanelConfig with auth mode, ports, bind address"
```

---

### Task 3: Event Bus (tokio::broadcast)

**Files:**
- Create: `src/api/events.rs`

**Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_bus_send_receive() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        bus.send(PanelEvent::ChannelStatus {
            channel: "telegram".into(),
            status: "up".into(),
        });
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, PanelEvent::ChannelStatus { .. }));
    }

    #[tokio::test]
    async fn test_event_bus_multiple_subscribers() {
        let bus = EventBus::new(16);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        bus.send(PanelEvent::AgentStarted {
            session_key: "test".into(),
        });
        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }
}
```

**Step 2: Run tests — verify fail**

Run: `cargo test --lib api::events`
Expected: FAIL

**Step 3: Implement EventBus**

```rust
//! Panel event bus — bridges agent loop events to WebSocket clients.

use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PanelEvent {
    ToolStarted { tool: String },
    ToolDone { tool: String, duration_ms: u64 },
    ToolFailed { tool: String, error: String },
    MessageReceived { channel: String, chat_id: String },
    AgentStarted { session_key: String },
    AgentDone { session_key: String, tokens: u64 },
    Compaction { from_tokens: u64, to_tokens: u64 },
    ChannelStatus { channel: String, status: String },
    CronFired { job_id: String, status: String },
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<PanelEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn send(&self, event: PanelEvent) {
        // Ignore error (no receivers is OK)
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PanelEvent> {
        self.tx.subscribe()
    }
}
```

**Step 4: Export from `src/api/mod.rs`**

```rust
pub mod config;
pub mod events;
```

**Step 5: Run tests**

Run: `cargo test --lib api::events`
Expected: PASS

**Step 6: Commit**

```bash
git add src/api/events.rs src/api/mod.rs
git commit -m "feat(panel): add EventBus with broadcast channel for real-time events"
```

---

### Task 4: Auth Middleware

**Files:**
- Create: `src/api/auth.rs`

**Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_bearer_token_valid() {
        let result = verify_bearer_token("Bearer abc123", "abc123");
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_bearer_token_invalid() {
        let result = verify_bearer_token("Bearer wrong", "abc123");
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_bearer_token_missing_prefix() {
        let result = verify_bearer_token("abc123", "abc123");
        assert!(result.is_err());
    }

    #[test]
    fn test_hash_and_verify_password() {
        let hash = hash_password("secret123").unwrap();
        assert!(verify_password("secret123", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }

    #[test]
    fn test_generate_jwt_and_validate() {
        let secret = "test-secret-key-32-bytes-long!!";
        let token = generate_jwt("admin", secret, 3600).unwrap();
        let claims = validate_jwt(&token, secret).unwrap();
        assert_eq!(claims.sub, "admin");
    }

    #[test]
    fn test_generate_api_token() {
        let token = generate_api_token();
        assert_eq!(token.len(), 64); // 32 bytes hex = 64 chars
    }
}
```

**Step 2: Run tests — verify fail**

Run: `cargo test --lib api::auth`

**Step 3: Implement auth functions**

```rust
//! Panel authentication — token, password, JWT helpers.

use crate::error::ZeptoError;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
}

pub fn generate_api_token() -> String {
    use std::fmt::Write;
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("failed to generate random bytes");
    let mut hex = String::with_capacity(64);
    for b in &bytes {
        write!(hex, "{b:02x}").unwrap();
    }
    hex
}

pub fn verify_bearer_token(header: &str, expected: &str) -> Result<(), ZeptoError> {
    let token = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| ZeptoError::Auth("Missing Bearer prefix".into()))?;
    if token == expected {
        Ok(())
    } else {
        Err(ZeptoError::Auth("Invalid token".into()))
    }
}

pub fn hash_password(password: &str) -> Result<String, ZeptoError> {
    bcrypt::hash(password, 12).map_err(|e| ZeptoError::Config(e.to_string()))
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool, ZeptoError> {
    bcrypt::verify(password, hash).map_err(|e| ZeptoError::Config(e.to_string()))
}

pub fn generate_jwt(username: &str, secret: &str, expires_in_secs: u64) -> Result<String, ZeptoError> {
    let exp = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() + expires_in_secs) as usize;
    let claims = Claims { sub: username.to_string(), exp };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_ref()))
        .map_err(|e| ZeptoError::Auth(e.to_string()))
}

pub fn validate_jwt(token: &str, secret: &str) -> Result<Claims, ZeptoError> {
    let data = decode::<Claims>(token, &DecodingKey::from_secret(secret.as_ref()), &Validation::default())
        .map_err(|e| ZeptoError::Auth(e.to_string()))?;
    Ok(data.claims)
}
```

Note: check if `getrandom` is already a dep; if not, use `rand::thread_rng().fill_bytes()` or add `getrandom`.

**Step 4: Export from `src/api/mod.rs`**

```rust
pub mod auth;
pub mod config;
pub mod events;
```

**Step 5: Run tests**

Run: `cargo test --lib api::auth`
Expected: PASS

**Step 6: Commit**

```bash
git add src/api/auth.rs src/api/mod.rs
git commit -m "feat(panel): add auth helpers — token, bcrypt password, JWT"
```

---

### Task 5: Axum API Server Skeleton

**Files:**
- Create: `src/api/server.rs`
- Create: `src/api/routes/mod.rs`
- Create: `src/api/routes/health.rs`

**Step 1: Write test for server startup**

In `src/api/server.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_build_router() {
        let event_bus = EventBus::new(16);
        let state = AppState::new("test-token".into(), event_bus);
        let app = build_router(state, None);
        // Verify router builds without panic
        assert!(true);
    }
}
```

**Step 2: Run test — verify fail**

Run: `cargo test --lib api::server`

**Step 3: Implement server skeleton**

`src/api/server.rs`:
```rust
//! Axum API server for ZeptoClaw Panel.

use crate::api::config::PanelConfig;
use crate::api::events::EventBus;
use axum::{extract::State, routing::get, Router};
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

#[derive(Clone)]
pub struct AppState {
    pub api_token: String,
    pub event_bus: EventBus,
}

impl AppState {
    pub fn new(api_token: String, event_bus: EventBus) -> Self {
        Self { api_token, event_bus }
    }
}

pub fn build_router(state: AppState, static_dir: Option<PathBuf>) -> Router {
    let api = Router::new()
        .route("/api/health", get(super::routes::health::get_health))
        .layer(
            CorsLayer::new()
                .allow_origin(Any) // Tightened per-config at runtime
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(Arc::new(state));

    if let Some(dir) = static_dir {
        api.fallback_service(tower_http::services::ServeDir::new(dir))
    } else {
        api
    }
}

pub async fn start_server(config: &PanelConfig, state: AppState, static_dir: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let app = build_router(state, static_dir);
    let addr = format!("{}:{}", config.bind, config.api_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Panel API server listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
```

`src/api/routes/mod.rs`:
```rust
pub mod health;
```

`src/api/routes/health.rs`:
```rust
use axum::Json;
use serde_json::{json, Value};

pub async fn get_health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
```

**Step 4: Update `src/api/mod.rs`**

```rust
pub mod auth;
pub mod config;
pub mod events;
pub mod routes;
pub mod server;
```

**Step 5: Run tests**

Run: `cargo test --lib api::server`
Expected: PASS

Run: `cargo clippy -- -D warnings`
Expected: PASS

**Step 6: Commit**

```bash
git add src/api/
git commit -m "feat(panel): axum API server skeleton with health route and static serving"
```

---

### Task 6: WebSocket Event Streaming

**Files:**
- Create: `src/api/routes/ws.rs`
- Modify: `src/api/routes/mod.rs`
- Modify: `src/api/server.rs`

**Step 1: Implement WebSocket handler**

`src/api/routes/ws.rs`:
```rust
use crate::api::events::{EventBus, PanelEvent};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use std::sync::Arc;

pub async fn ws_events(
    ws: WebSocketUpgrade,
    State(state): State<Arc<super::super::server::AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state.event_bus.clone()))
}

async fn handle_ws(mut socket: WebSocket, event_bus: EventBus) {
    let mut rx = event_bus.subscribe();
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(e) => {
                        let json = serde_json::to_string(&e).unwrap_or_default();
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            break; // Client disconnected
                        }
                    }
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // Ignore client messages
                }
            }
        }
    }
}
```

**Step 2: Wire into router in `src/api/server.rs`**

Add route: `.route("/ws/events", get(super::routes::ws::ws_events))`

**Step 3: Update `src/api/routes/mod.rs`**

```rust
pub mod health;
pub mod ws;
```

**Step 4: Run clippy + tests**

Run: `cargo clippy -- -D warnings && cargo test --lib`

**Step 5: Commit**

```bash
git add src/api/routes/ws.rs src/api/routes/mod.rs src/api/server.rs
git commit -m "feat(panel): WebSocket event streaming from EventBus to clients"
```

---

### Task 7: REST Routes — Sessions, Channels, Cron, Routines

**Files:**
- Create: `src/api/routes/sessions.rs`
- Create: `src/api/routes/channels.rs`
- Create: `src/api/routes/cron.rs`
- Create: `src/api/routes/routines.rs`
- Create: `src/api/routes/metrics.rs`
- Modify: `src/api/routes/mod.rs`
- Modify: `src/api/server.rs`

Each route module follows the same pattern:

```rust
// Example: src/api/routes/sessions.rs
use axum::{extract::State, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::api::server::AppState;

pub async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<Value> {
    // Read from SessionManager on AppState
    Json(json!({ "sessions": [] }))
}

pub async fn get_session(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> Json<Value> {
    Json(json!({ "key": key, "messages": [] }))
}

pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> axum::http::StatusCode {
    axum::http::StatusCode::NO_CONTENT
}
```

**Routes to wire in `server.rs`:**
```rust
.route("/api/sessions", get(routes::sessions::list_sessions))
.route("/api/sessions/:key", get(routes::sessions::get_session).delete(routes::sessions::delete_session))
.route("/api/channels", get(routes::channels::list_channels))
.route("/api/cron", get(routes::cron::list_jobs).post(routes::cron::create_job))
.route("/api/cron/:id", put(routes::cron::update_job).delete(routes::cron::delete_job))
.route("/api/cron/:id/trigger", post(routes::cron::trigger_job))
.route("/api/routines", get(routes::routines::list_routines).post(routes::routines::create_routine))
.route("/api/routines/:id", put(routes::routines::update_routine).delete(routes::routines::delete_routine))
.route("/api/routines/:id/toggle", post(routes::routines::toggle_routine))
.route("/api/metrics", get(routes::metrics::get_metrics))
```

**Implementation note:** Start with stub responses returning `[]` / `{}`. Wire to real data stores (SessionManager, CronStore, RoutineStore) in a later task by adding them to `AppState`.

Write unit tests for each route using `axum::test` or direct handler calls.

**Commit per route file**, or batch:

```bash
git commit -m "feat(panel): REST route stubs for sessions, channels, cron, routines, metrics"
```

---

### Task 8: Kanban Task Model + TaskTool

**Files:**
- Create: `src/api/routes/tasks.rs`
- Create: `src/tools/task.rs`
- Modify: `src/tools/mod.rs`
- Modify: `src/api/server.rs`

**Step 1: Write tests for TaskStore**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_list_tasks() {
        let store = TaskStore::new_in_memory();
        store.create("Fix bug", "backlog", None).await.unwrap();
        let tasks = store.list(None).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].column, "backlog");
    }

    #[tokio::test]
    async fn test_move_task() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Ship it", "backlog", None).await.unwrap();
        store.move_task(&id, "in_progress").await.unwrap();
        let task = store.get(&id).await.unwrap().unwrap();
        assert_eq!(task.column, "in_progress");
    }
}
```

**Step 2: Implement TaskStore + KanbanTask model**

```rust
//! Kanban task model — persisted at ~/.zeptoclaw/tasks.json

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanTask {
    pub id: String,
    pub title: String,
    pub description: String,
    pub column: String,       // backlog, in_progress, review, done
    pub assignee: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct TaskStore {
    tasks: Arc<RwLock<HashMap<String, KanbanTask>>>,
    path: Option<std::path::PathBuf>,
}
```

**Step 3: Implement TaskTool (agent-accessible)**

Follow pattern from `src/tools/longterm_memory.rs`:
```rust
pub struct TaskTool {
    store: Arc<TaskStore>,
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str { "task" }
    fn description(&self) -> &str { "Manage kanban board tasks (create, update, move, list, delete)" }
    // actions: create, update, move, list, delete
}
```

**Step 4: Register TaskTool in `src/cli/common.rs`**

Add: `agent.register_tool(Box::new(TaskTool::new(task_store.clone()))).await;`

**Step 5: Add REST routes**

```rust
.route("/api/tasks", get(routes::tasks::list_tasks).post(routes::tasks::create_task))
.route("/api/tasks/:id", put(routes::tasks::update_task).delete(routes::tasks::delete_task))
.route("/api/tasks/:id/move", post(routes::tasks::move_task))
```

**Step 6: Run tests**

Run: `cargo test --lib api::routes::tasks && cargo test --lib tools::task`

**Step 7: Commit**

```bash
git commit -m "feat(panel): kanban TaskStore + TaskTool + REST routes"
```

---

### Task 9: CLI `panel` Command

**Files:**
- Create: `src/cli/panel.rs`
- Modify: `src/cli/mod.rs` (add `Panel` to `Commands` enum + dispatch)

**Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_panel_dir_from_repo() {
        // Create temp dir with panel/dist/index.html
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("panel/dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("index.html"), "<html>").unwrap();
        let result = resolve_panel_dir(Some(dir.path().to_path_buf()));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_panel_dir_missing() {
        let result = resolve_panel_dir(Some("/nonexistent".into()));
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_generate_and_load_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panel.token");
        let token = ensure_api_token(&path).await.unwrap();
        assert_eq!(token.len(), 64);
        // Second call should return same token
        let token2 = ensure_api_token(&path).await.unwrap();
        assert_eq!(token, token2);
    }
}
```

**Step 2: Implement panel command**

```rust
//! `zeptoclaw panel` command — install, start, auth management.

use crate::api::auth::generate_api_token;
use crate::api::config::PanelConfig;
use crate::api::events::EventBus;
use crate::api::server::{start_server, AppState};
use crate::config::Config;
use crate::error::ZeptoError;
use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub enum PanelAction {
    /// Install panel (build from source or download pre-built)
    Install {
        #[arg(long)]
        download: bool,
        #[arg(long)]
        rebuild: bool,
    },
    /// Manage authentication
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Uninstall panel (remove dist, node_modules, token)
    Uninstall,
}

#[derive(clap::Subcommand)]
pub enum AuthAction {
    Mode { mode: String },
    ResetPassword,
    Status,
}

pub async fn cmd_panel(
    config: Config,
    action: Option<PanelAction>,
    dev: bool,
    api_only: bool,
    port: Option<u16>,
    api_port: Option<u16>,
    rotate_token: bool,
) -> Result<(), ZeptoError> {
    match action {
        Some(PanelAction::Install { download, rebuild }) => cmd_install(download, rebuild).await,
        Some(PanelAction::Auth { action }) => cmd_auth(action).await,
        Some(PanelAction::Uninstall) => cmd_uninstall().await,
        None => cmd_start(config, dev, api_only, port, api_port, rotate_token).await,
    }
}
```

**Step 3: Add to Commands enum in `src/cli/mod.rs`**

```rust
/// Start the control panel
Panel {
    #[command(subcommand)]
    action: Option<panel::PanelAction>,
    /// Dev mode (API only, run pnpm dev separately)
    #[arg(long)]
    dev: bool,
    /// API server only, no static file serving
    #[arg(long)]
    api_only: bool,
    /// Panel port (default: 9092)
    #[arg(long)]
    port: Option<u16>,
    /// API port (default: 9091)
    #[arg(long)]
    api_port: Option<u16>,
    /// Regenerate API token
    #[arg(long)]
    rotate_token: bool,
},
```

Add dispatch:
```rust
Some(Commands::Panel { action, dev, api_only, port, api_port, rotate_token }) => {
    panel::cmd_panel(config, action, dev, api_only, port, api_port, rotate_token).await?
}
```

**Step 4: Run tests**

Run: `cargo test --lib cli::panel`

**Step 5: Commit**

```bash
git commit -m "feat(panel): CLI 'zeptoclaw panel' command with install/start/auth/uninstall"
```

---

### Task 10: Wire EventBus into Agent Loop

**Files:**
- Modify: `src/agent/loop.rs` — emit `PanelEvent`s for tool start/done/fail
- Modify: `src/cli/common.rs` — pass `EventBus` to `AgentLoop`

**Step 1: Add `Option<EventBus>` field to AgentLoop**

In `src/agent/loop.rs`, add to the struct:
```rust
pub event_bus: Option<crate::api::events::EventBus>,
```

**Step 2: Emit events around tool execution**

Find the tool execution section in the agent loop. Before tool `execute()`:
```rust
if let Some(bus) = &self.event_bus {
    bus.send(PanelEvent::ToolStarted { tool: tool_name.clone() });
}
```

After tool execution (on success):
```rust
if let Some(bus) = &self.event_bus {
    bus.send(PanelEvent::ToolDone { tool: tool_name.clone(), duration_ms: elapsed.as_millis() as u64 });
}
```

On tool error:
```rust
if let Some(bus) = &self.event_bus {
    bus.send(PanelEvent::ToolFailed { tool: tool_name.clone(), error: err.to_string() });
}
```

**Step 3: Pass EventBus from gateway/CLI when panel is enabled**

In `src/cli/common.rs` `create_agent_with_template()`, accept optional `EventBus` param and set it on the agent loop.

**Step 4: Run tests — make sure existing tests still pass**

Run: `cargo test --lib`

**Step 5: Commit**

```bash
git commit -m "feat(panel): emit PanelEvents from agent loop tool execution"
```

---

## Phase 2: Panel Frontend

### Task 11: Scaffold Vite + React + Tailwind

**Files:**
- Create: `panel/` directory with full Vite scaffold

**Step 1: Initialize project**

```bash
cd /Users/dr.noranizaahmad/ios/zeptoclaw
pnpm create vite panel --template react-ts
cd panel
pnpm install
pnpm add -D tailwindcss @tailwindcss/vite
pnpm add react-router @tanstack/react-query recharts @dnd-kit/core @dnd-kit/sortable
```

**Step 2: Configure Vite proxy**

`panel/vite.config.ts`:
```typescript
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 9092,
    proxy: {
      '/api': 'http://localhost:9091',
      '/ws': { target: 'ws://localhost:9091', ws: true },
    },
  },
})
```

**Step 3: Add to `.gitignore`**

```
panel/node_modules/
panel/dist/
```

**Step 4: Verify dev server starts**

```bash
cd panel && pnpm dev
```

**Step 5: Commit**

```bash
git add panel/ .gitignore
git commit -m "feat(panel): scaffold Vite + React + Tailwind frontend"
```

---

### Task 12: Layout Shell + Router

**Files:**
- Create: `panel/src/App.tsx`
- Create: `panel/src/components/Sidebar.tsx`
- Create: `panel/src/components/Layout.tsx`
- Create: `panel/src/pages/Dashboard.tsx` (stub)
- Create: `panel/src/pages/Logs.tsx` (stub)
- Create: `panel/src/pages/Sessions.tsx` (stub)
- Create: `panel/src/pages/CronRoutines.tsx` (stub)
- Create: `panel/src/pages/Kanban.tsx` (stub)
- Create: `panel/src/pages/Agents.tsx` (stub)

Implement sidebar nav with react-router. Each page is a stub with just the page title.

Dark theme via Tailwind: `bg-zinc-950 text-zinc-100`.

**Commit:**
```bash
git commit -m "feat(panel): layout shell with sidebar nav and 6 page stubs"
```

---

### Task 13: Dashboard Page

**Files:**
- Modify: `panel/src/pages/Dashboard.tsx`
- Create: `panel/src/hooks/useHealth.ts`
- Create: `panel/src/hooks/useWebSocket.ts`
- Create: `panel/src/lib/api.ts`

Implement:
- `useHealth()` hook — polls `GET /api/health` every 5s via react-query
- `useWebSocket()` hook — connects to `/ws/events`, returns event stream
- Health status pill, uptime, RSS, version
- Channel status cards
- Mini activity feed (last 10 events from WebSocket)
- Token usage + cost display from `/api/metrics`

**Commit:**
```bash
git commit -m "feat(panel): dashboard page with health, channels, activity feed"
```

---

### Task 14: Logs Page

**Files:**
- Modify: `panel/src/pages/Logs.tsx`

Implement:
- Live-scrolling list consuming WebSocket events
- Filter chips: tool, agent, channel, cron
- Pause/resume button
- Color-coded rows (green/red/blue)
- Expandable error details
- Auto-scroll with "jump to bottom" button

**Commit:**
```bash
git commit -m "feat(panel): real-time logs page with filters and auto-scroll"
```

---

### Task 15: Sessions Page

**Files:**
- Modify: `panel/src/pages/Sessions.tsx`
- Create: `panel/src/components/ChatBubble.tsx`
- Create: `panel/src/components/ToolCallBlock.tsx`

Implement:
- Session list from `GET /api/sessions` with search
- Chat bubble renderer (user/assistant roles)
- Tool calls as collapsible blocks
- Per-session stats bar

**Commit:**
```bash
git commit -m "feat(panel): sessions page with chat viewer and tool call display"
```

---

### Task 16: Cron & Routines Page

**Files:**
- Modify: `panel/src/pages/CronRoutines.tsx`

Implement:
- Tab navigation: Cron | Routines
- Cron: list from `GET /api/cron`, create/edit modal, manual trigger button
- Routines: list from `GET /api/routines`, toggle switch, trigger type badges
- Cron expression preview (human-readable next 5 runs)

**Commit:**
```bash
git commit -m "feat(panel): cron and routines management page with create/edit/toggle"
```

---

### Task 17: Kanban Page

**Files:**
- Modify: `panel/src/pages/Kanban.tsx`
- Create: `panel/src/components/KanbanColumn.tsx`
- Create: `panel/src/components/KanbanCard.tsx`

Implement:
- 4 columns: Backlog, In Progress, Review, Done
- `@dnd-kit` drag-and-drop between columns
- Card component: title, assignee badge (human/agent), priority color
- Create task modal
- Filter bar: assignee, label, priority
- Mutations via `POST/PUT /api/tasks`

**Commit:**
```bash
git commit -m "feat(panel): kanban board with drag-and-drop and task CRUD"
```

---

### Task 18: Agents Page

**Files:**
- Modify: `panel/src/pages/Agents.tsx`
- Create: `panel/src/components/AgentDesk.tsx`

Implement:
- Grid of "agent desks" — one per active session from WebSocket
- Each desk: channel icon, current tool animation, token counter
- Idle = dimmed, active = highlighted with pulse
- Click → navigate to that session's logs
- Swarm tree view when DelegateTool events detected

**Commit:**
```bash
git commit -m "feat(panel): live agent office with desk grid and swarm visualization"
```

---

## Phase 3: Polish & Integration

### Task 19: Auth Token Flow for Frontend

**Files:**
- Create: `panel/src/hooks/useAuth.ts`
- Create: `panel/src/pages/Login.tsx`
- Modify: `panel/src/App.tsx` (add auth guard)
- Modify: `src/api/server.rs` (add auth middleware)

Implement:
- Auth middleware in axum: check `Authorization: Bearer` header on all `/api/*` routes
- Login page (only shown in `password` mode)
- `useAuth()` hook: stores JWT in memory (not localStorage), refreshes on expiry
- Token mode: read from `~/.zeptoclaw/panel.token`, inject via initial handshake
- Protected route wrapper in App.tsx

**Commit:**
```bash
git commit -m "feat(panel): auth middleware + login page + token handshake"
```

---

### Task 20: CSRF + Security Hardening

**Files:**
- Modify: `src/api/server.rs`
- Create: `src/api/middleware.rs`

Implement:
- CSRF token endpoint: `GET /api/csrf-token`
- Validate `X-CSRF-Token` on all POST/PUT/DELETE
- Rate limiting via `tower::limit::RateLimitLayer`
- WebSocket connection cap (5 max via semaphore)
- Request body size limit (1MB via `axum::extract::DefaultBodyLimit`)
- Tighten CORS to only `http://localhost:{panel_port}`

**Commit:**
```bash
git commit -m "feat(panel): CSRF protection, rate limiting, CORS lockdown"
```

---

### Task 21: `panel install` Implementation

**Files:**
- Modify: `src/cli/panel.rs`

Implement `cmd_install()`:
1. Check `node --version` (>= 18)
2. Check `pnpm --version` (run `corepack enable pnpm` if missing)
3. `pnpm install --dir panel/`
4. `pnpm --dir panel build`
5. `ensure_api_token()`
6. Interactive auth setup prompt (use `dialoguer` crate if available, else stdin)
7. Print success

Implement `cmd_install()` with `--download`:
1. Fetch `https://github.com/qhkm/zeptoclaw/releases/download/v{version}/panel-dist.tar.gz`
2. Extract to `~/.zeptoclaw/panel/dist/`
3. `ensure_api_token()`

**Commit:**
```bash
git commit -m "feat(panel): implement 'panel install' — build from source and download paths"
```

---

### Task 22: Wire Real Data into API Routes

**Files:**
- Modify: `src/api/server.rs` (add SessionManager, CronStore, RoutineStore, TaskStore to AppState)
- Modify: `src/api/routes/sessions.rs` (read from SessionManager)
- Modify: `src/api/routes/cron.rs` (read/write CronStore)
- Modify: `src/api/routes/routines.rs` (read/write RoutineStore)
- Modify: `src/api/routes/channels.rs` (read from ChannelManager/HealthRegistry)
- Modify: `src/api/routes/metrics.rs` (read from MetricsCollector + CostTracker)

This task replaces the stub responses with real data from ZeptoClaw's existing stores.

**AppState additions:**
```rust
pub struct AppState {
    pub api_token: String,
    pub event_bus: EventBus,
    pub session_manager: Option<Arc<SessionManager>>,
    pub cron_store: Option<Arc<CronStore>>,
    pub routine_store: Option<Arc<RoutineStore>>,
    pub task_store: Arc<TaskStore>,
    pub health_registry: Option<Arc<HealthRegistry>>,
    pub metrics_collector: Option<Arc<MetricsCollector>>,
    pub cost_tracker: Option<Arc<CostTracker>>,
}
```

**Commit:**
```bash
git commit -m "feat(panel): wire real data stores into API routes"
```

---

### Task 23: Update CLAUDE.md + AGENTS.md

**Files:**
- Modify: `CLAUDE.md` (add panel section to quick reference, architecture, CLI commands)
- Modify: `AGENTS.md` (if exists, add panel module docs)

Add:
- `zeptoclaw panel` commands to Quick Reference
- `src/api/` to Architecture section
- `panel/` to project structure
- New dependencies to Dependencies section
- `panel` config to Configuration section
- New env vars: `ZEPTOCLAW_PANEL_*`

**Commit:**
```bash
git commit -m "docs: update CLAUDE.md and AGENTS.md for control panel"
```

---

## Summary

| Phase | Tasks | What Ships |
|-------|-------|------------|
| Phase 1: API Foundation | Tasks 1–10 | axum server, EventBus, auth, REST stubs, WebSocket, TaskTool, CLI command, agent loop instrumentation |
| Phase 2: Frontend | Tasks 11–18 | All 6 pages: Dashboard, Logs, Sessions, Cron/Routines, Kanban, Agents |
| Phase 3: Polish | Tasks 19–23 | Auth flow, CSRF, security hardening, install command, real data wiring, docs |

**Total: 23 tasks across 3 phases**

**Binary size impact:** ~450KB (axum + tower-http + jwt + bcrypt)

**New Cargo deps:** `axum`, `tower-http`, `jsonwebtoken`, `bcrypt`

**New npm deps:** `react-router`, `@tanstack/react-query`, `recharts`, `@dnd-kit/core`, `@dnd-kit/sortable`
