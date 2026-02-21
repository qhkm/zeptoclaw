//! Heartbeat service - periodic agent wake-up for background tasks.

mod service;
mod template;

pub use service::{HeartbeatResult, HeartbeatService, HEARTBEAT_PROMPT};
pub use template::{ensure_heartbeat_file, HEARTBEAT_TEMPLATE};
