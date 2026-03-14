//! r8r bridge integration for ZeptoClaw.
//!
//! This module handles the bidirectional event stream between ZeptoClaw and
//! [r8r](https://github.com/qhkm/r8r), the workflow-automation engine.
//!
//! # Submodules
//!
//! * [`events`] — Mirrored event types matching r8r's wire format.
//! * [`dedup`]  — Deduplicator for at-least-once delivery.
//! * [`approval`] — Approval routing and response parsing (Task 8).
//! * [`health`] — Health ping loop and CLI status (Task 9).

pub mod approval;
pub mod dedup;
pub mod events;
pub mod health;

pub use dedup::Deduplicator;
pub use events::{Ack, BridgeEvent, BridgeEventEnvelope};
