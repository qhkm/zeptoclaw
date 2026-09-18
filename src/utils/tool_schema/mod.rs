//! Tool JSON-Schema repair for strict and local LLM backends.
//!
//! Two halves of one round trip:
//! - [`sanitize`] rewrites outbound tool schemas into shapes strict backends
//!   and llama.cpp's grammar converter accept, renaming illegal property keys.
//! - [`coerce`] repairs inbound model-emitted argument values against the
//!   tool's schema, after [`sanitize::unrename_tool_args`] restores wire keys.

pub mod coerce;
pub mod sanitize;

pub use coerce::coerce_tool_args;
pub use sanitize::{sanitize_property_key, sanitize_schema, unrename_tool_args};
