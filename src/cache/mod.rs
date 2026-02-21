//! LLM response caching with TTL, LRU eviction, and JSON persistence.

pub mod response_cache;

pub use response_cache::{CacheStats, ResponseCache};
