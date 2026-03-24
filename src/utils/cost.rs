//! LLM API cost estimation and tracking.
//!
//! Provides model pricing data, per-call cost estimation, and a thread-safe
//! `CostTracker` that accumulates spend across providers and models within
//! a session. Uses interior mutability via `Mutex` so all recording methods
//! take `&self`.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Pricing for a single LLM model, expressed in USD per million tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Cost per 1 000 000 input (prompt) tokens in USD.
    pub input_cost_per_million: f64,
    /// Cost per 1 000 000 output (completion) tokens in USD.
    pub output_cost_per_million: f64,
}

/// Returns a static map of known model pricing.
///
/// Prices are in USD per million tokens and reflect public list prices at
/// the time of writing.
pub fn default_pricing() -> HashMap<String, ModelPricing> {
    let mut m = HashMap::new();

    // Anthropic Claude models
    m.insert(
        "claude-sonnet-4-6".to_string(),
        ModelPricing {
            input_cost_per_million: 3.0,
            output_cost_per_million: 15.0,
        },
    );
    m.insert(
        "claude-sonnet-4-5-20250929".to_string(),
        ModelPricing {
            input_cost_per_million: 3.0,
            output_cost_per_million: 15.0,
        },
    );
    m.insert(
        "claude-3-5-sonnet-20241022".to_string(),
        ModelPricing {
            input_cost_per_million: 3.0,
            output_cost_per_million: 15.0,
        },
    );
    m.insert(
        "claude-opus-4-6".to_string(),
        ModelPricing {
            input_cost_per_million: 15.0,
            output_cost_per_million: 75.0,
        },
    );
    m.insert(
        "claude-3-opus-20240229".to_string(),
        ModelPricing {
            input_cost_per_million: 15.0,
            output_cost_per_million: 75.0,
        },
    );
    m.insert(
        "claude-3-haiku-20240307".to_string(),
        ModelPricing {
            input_cost_per_million: 0.25,
            output_cost_per_million: 1.25,
        },
    );

    // OpenAI models
    m.insert(
        "gpt-5.1".to_string(),
        ModelPricing {
            input_cost_per_million: 2.5,
            output_cost_per_million: 10.0,
        },
    );
    m.insert(
        "gpt-4o-mini".to_string(),
        ModelPricing {
            input_cost_per_million: 0.15,
            output_cost_per_million: 0.6,
        },
    );
    m.insert(
        "gpt-4-turbo".to_string(),
        ModelPricing {
            input_cost_per_million: 10.0,
            output_cost_per_million: 30.0,
        },
    );

    m
}

/// Estimate the cost of a single LLM call in USD.
///
/// Looks up pricing in `custom_pricing` first, then falls back to
/// [`default_pricing`]. Returns `None` if the model is unknown in both.
pub fn estimate_cost(
    model: &str,
    prompt_tokens: u32,
    completion_tokens: u32,
    custom_pricing: &HashMap<String, ModelPricing>,
) -> Option<f64> {
    // Resolve the lookup in two steps so that the owned `defaults` HashMap
    // lives long enough for the borrow returned by `.get()`.
    let defaults = default_pricing();
    let pricing = custom_pricing.get(model).or_else(|| defaults.get(model));

    pricing.map(|p| {
        let input_cost = (prompt_tokens as f64 / 1_000_000.0) * p.input_cost_per_million;
        let output_cost = (completion_tokens as f64 / 1_000_000.0) * p.output_cost_per_million;
        input_cost + output_cost
    })
}

/// Internal mutable state guarded by the `CostTracker` mutex.
#[derive(Debug, Default)]
struct CostState {
    total_cost: f64,
    per_provider: HashMap<String, f64>,
    per_model: HashMap<String, f64>,
    call_count: u64,
}

/// Thread-safe, session-level cost accumulator.
///
/// All recording methods take `&self` (interior mutability via `Mutex`),
/// making it easy to share across async tasks via `Arc<CostTracker>`.
#[derive(Debug)]
pub struct CostTracker {
    state: Mutex<CostState>,
    custom_pricing: HashMap<String, ModelPricing>,
}

impl CostTracker {
    /// Creates a new tracker with default model pricing only.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(CostState::default()),
            custom_pricing: HashMap::new(),
        }
    }

    /// Creates a new tracker with additional custom model pricing.
    ///
    /// Custom entries take precedence over the built-in defaults.
    pub fn new_with_pricing(custom: HashMap<String, ModelPricing>) -> Self {
        Self {
            state: Mutex::new(CostState::default()),
            custom_pricing: custom,
        }
    }

    /// Record a single LLM call.
    ///
    /// Estimates cost (if the model is known) and accumulates it under both
    /// the provider name and the model name.
    pub fn record(&self, provider: &str, model: &str, prompt_tokens: u32, completion_tokens: u32) {
        let cost = estimate_cost(
            model,
            prompt_tokens,
            completion_tokens,
            &self.custom_pricing,
        )
        .unwrap_or(0.0);

        let mut state = self.state.lock().unwrap();
        state.total_cost += cost;
        *state
            .per_provider
            .entry(provider.to_string())
            .or_insert(0.0) += cost;
        *state.per_model.entry(model.to_string()).or_insert(0.0) += cost;
        state.call_count += 1;
    }

    /// Returns the total accumulated cost in USD.
    pub fn total_cost(&self) -> f64 {
        self.state.lock().unwrap().total_cost
    }

    /// Returns a snapshot of accumulated cost per provider.
    pub fn cost_by_provider(&self) -> HashMap<String, f64> {
        self.state.lock().unwrap().per_provider.clone()
    }

    /// Returns a snapshot of accumulated cost per model.
    pub fn cost_by_model(&self) -> HashMap<String, f64> {
        self.state.lock().unwrap().per_model.clone()
    }

    /// Returns the total number of LLM calls recorded.
    pub fn call_count(&self) -> u64 {
        self.state.lock().unwrap().call_count
    }

    /// Produces a human-readable cost summary.
    ///
    /// Example output:
    /// ```text
    /// Total: $0.0150 (3 calls) | anthropic: $0.0120, openai: $0.0030
    /// ```
    pub fn summary(&self) -> String {
        let state = self.state.lock().unwrap();

        let mut summary = format!(
            "Total: ${:.4} ({} calls)",
            state.total_cost, state.call_count,
        );

        if !state.per_provider.is_empty() {
            let mut providers: Vec<_> = state.per_provider.iter().collect();
            providers.sort_by(|a, b| a.0.cmp(b.0));

            let parts: Vec<String> = providers
                .iter()
                .map(|(name, cost)| format!("{}: ${:.4}", name, cost))
                .collect();

            summary.push_str(" | ");
            summary.push_str(&parts.join(", "));
        }

        summary
    }
}

impl Default for CostTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for cost tracking, suitable for embedding in the main
/// ZeptoClaw config file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CostConfig {
    /// Whether cost tracking is enabled.
    pub enabled: bool,
    /// Custom per-model pricing overrides.
    pub custom_pricing: HashMap<String, ModelPricing>,
}

// We need Copy-like semantics for the lookup in estimate_cost where we clone
// out of a temporary HashMap. Derive Copy if the fields allow it (f64 is Copy).
impl Copy for ModelPricing {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_pricing_contains_claude_sonnet() {
        let prices = default_pricing();
        assert!(prices.contains_key("claude-sonnet-4-5-20250929"));
        let p = &prices["claude-sonnet-4-5-20250929"];
        assert!((p.input_cost_per_million - 3.0).abs() < f64::EPSILON);
        assert!((p.output_cost_per_million - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_default_pricing_contains_all_expected_models() {
        let prices = default_pricing();
        let expected = [
            "claude-sonnet-4-6",
            "claude-sonnet-4-5-20250929",
            "claude-3-5-sonnet-20241022",
            "claude-opus-4-6",
            "claude-3-opus-20240229",
            "claude-3-haiku-20240307",
            "gpt-5.1",
            "gpt-4o-mini",
            "gpt-4-turbo",
        ];
        for model in &expected {
            assert!(prices.contains_key(*model), "missing model: {}", model);
        }
        assert_eq!(prices.len(), expected.len());
    }

    #[test]
    fn test_estimate_cost_known_model() {
        let custom = HashMap::new();
        // claude-sonnet-4-5: $3/M input, $15/M output
        // 1000 input tokens = 1000/1_000_000 * 3.0 = 0.003
        // 500 output tokens  = 500/1_000_000 * 15.0 = 0.0075
        let cost = estimate_cost("claude-sonnet-4-5-20250929", 1000, 500, &custom).unwrap();
        assert!((cost - 0.0105).abs() < 1e-10);
    }

    #[test]
    fn test_estimate_cost_gpt4o() {
        let custom = HashMap::new();
        // gpt-5.1: $2.5/M input, $10/M output
        // 2000 input  = 2000/1_000_000 * 2.5 = 0.005
        // 1000 output = 1000/1_000_000 * 10  = 0.01
        let cost = estimate_cost("gpt-5.1", 2000, 1000, &custom).unwrap();
        assert!((cost - 0.015).abs() < 1e-10);
    }

    #[test]
    fn test_estimate_cost_unknown_model_returns_none() {
        let custom = HashMap::new();
        assert!(estimate_cost("unknown-model-xyz", 1000, 500, &custom).is_none());
    }

    #[test]
    fn test_estimate_cost_custom_pricing_overrides_default() {
        let mut custom = HashMap::new();
        custom.insert(
            "gpt-5.1".to_string(),
            ModelPricing {
                input_cost_per_million: 100.0,
                output_cost_per_million: 200.0,
            },
        );
        // With custom pricing: 1000/1M * 100 + 500/1M * 200 = 0.1 + 0.1 = 0.2
        let cost = estimate_cost("gpt-5.1", 1000, 500, &custom).unwrap();
        assert!((cost - 0.2).abs() < 1e-10);
    }

    #[test]
    fn test_estimate_cost_custom_new_model() {
        let mut custom = HashMap::new();
        custom.insert(
            "my-custom-model".to_string(),
            ModelPricing {
                input_cost_per_million: 1.0,
                output_cost_per_million: 2.0,
            },
        );
        let cost = estimate_cost("my-custom-model", 1_000_000, 1_000_000, &custom).unwrap();
        assert!((cost - 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_cost_tracker_new_starts_at_zero() {
        let tracker = CostTracker::new();
        assert!((tracker.total_cost() - 0.0).abs() < f64::EPSILON);
        assert_eq!(tracker.call_count(), 0);
        assert!(tracker.cost_by_provider().is_empty());
        assert!(tracker.cost_by_model().is_empty());
    }

    #[test]
    fn test_cost_tracker_record_accumulates() {
        let tracker = CostTracker::new();
        // gpt-5.1: $2.5/M input, $10/M output
        tracker.record("openai", "gpt-5.1", 1000, 500);
        // 1000/1M * 2.5 + 500/1M * 10 = 0.0025 + 0.005 = 0.0075
        assert!((tracker.total_cost() - 0.0075).abs() < 1e-10);
        assert_eq!(tracker.call_count(), 1);

        tracker.record("openai", "gpt-5.1", 1000, 500);
        assert!((tracker.total_cost() - 0.015).abs() < 1e-10);
        assert_eq!(tracker.call_count(), 2);
    }

    #[test]
    fn test_cost_tracker_multiple_providers() {
        let tracker = CostTracker::new();
        // anthropic call
        tracker.record("anthropic", "claude-sonnet-4-5-20250929", 1000, 500);
        // openai call
        tracker.record("openai", "gpt-5.1", 1000, 500);

        let by_provider = tracker.cost_by_provider();
        assert_eq!(by_provider.len(), 2);
        assert!(by_provider.contains_key("anthropic"));
        assert!(by_provider.contains_key("openai"));

        // anthropic: 1000/1M*3 + 500/1M*15 = 0.003 + 0.0075 = 0.0105
        assert!((by_provider["anthropic"] - 0.0105).abs() < 1e-10);
        // openai: 1000/1M*2.5 + 500/1M*10 = 0.0025 + 0.005 = 0.0075
        assert!((by_provider["openai"] - 0.0075).abs() < 1e-10);
    }

    #[test]
    fn test_cost_tracker_multiple_models() {
        let tracker = CostTracker::new();
        tracker.record("openai", "gpt-5.1", 1000, 500);
        tracker.record("openai", "gpt-4o-mini", 1000, 500);

        let by_model = tracker.cost_by_model();
        assert_eq!(by_model.len(), 2);
        assert!(by_model.contains_key("gpt-5.1"));
        assert!(by_model.contains_key("gpt-4o-mini"));

        // gpt-5.1: 0.0075
        assert!((by_model["gpt-5.1"] - 0.0075).abs() < 1e-10);
        // gpt-4o-mini: 1000/1M*0.15 + 500/1M*0.6 = 0.00015 + 0.0003 = 0.00045
        assert!((by_model["gpt-4o-mini"] - 0.00045).abs() < 1e-10);
    }

    #[test]
    fn test_cost_tracker_summary_format() {
        let tracker = CostTracker::new();
        tracker.record("anthropic", "claude-sonnet-4-5-20250929", 1000, 500);
        tracker.record("openai", "gpt-5.1", 2000, 1000);
        tracker.record("openai", "gpt-5.1", 2000, 1000);

        let summary = tracker.summary();

        assert!(summary.contains("Total: $"), "missing Total prefix");
        assert!(summary.contains("(3 calls)"), "missing call count");
        assert!(summary.contains("anthropic: $"), "missing anthropic");
        assert!(summary.contains("openai: $"), "missing openai");
    }

    #[test]
    fn test_cost_tracker_call_count() {
        let tracker = CostTracker::new();
        assert_eq!(tracker.call_count(), 0);

        tracker.record("anthropic", "claude-3-haiku-20240307", 100, 50);
        assert_eq!(tracker.call_count(), 1);

        tracker.record("anthropic", "claude-3-haiku-20240307", 100, 50);
        tracker.record("openai", "gpt-4o-mini", 100, 50);
        assert_eq!(tracker.call_count(), 3);
    }

    #[test]
    fn test_cost_tracker_unknown_model_zero_cost() {
        let tracker = CostTracker::new();
        tracker.record("custom", "unknown-model", 10000, 5000);

        // Unknown model should record 0.0 cost but still count the call
        assert!((tracker.total_cost() - 0.0).abs() < f64::EPSILON);
        assert_eq!(tracker.call_count(), 1);
        assert!(tracker.cost_by_provider().contains_key("custom"));
    }

    #[test]
    fn test_cost_config_default() {
        let config = CostConfig::default();
        assert!(!config.enabled);
        assert!(config.custom_pricing.is_empty());
    }

    #[test]
    fn test_cost_config_serde_roundtrip() {
        let mut custom = HashMap::new();
        custom.insert(
            "my-model".to_string(),
            ModelPricing {
                input_cost_per_million: 5.0,
                output_cost_per_million: 20.0,
            },
        );
        let config = CostConfig {
            enabled: true,
            custom_pricing: custom,
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: CostConfig = serde_json::from_str(&json).unwrap();

        assert!(parsed.enabled);
        assert_eq!(parsed.custom_pricing.len(), 1);
        let p = &parsed.custom_pricing["my-model"];
        assert!((p.input_cost_per_million - 5.0).abs() < f64::EPSILON);
        assert!((p.output_cost_per_million - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_model_pricing_serde_roundtrip() {
        let pricing = ModelPricing {
            input_cost_per_million: 3.0,
            output_cost_per_million: 15.0,
        };

        let json = serde_json::to_string(&pricing).unwrap();
        let parsed: ModelPricing = serde_json::from_str(&json).unwrap();

        assert!((parsed.input_cost_per_million - 3.0).abs() < f64::EPSILON);
        assert!((parsed.output_cost_per_million - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_cost_config_serde_defaults_on_missing_fields() {
        let json = "{}";
        let parsed: CostConfig = serde_json::from_str(json).unwrap();
        assert!(!parsed.enabled);
        assert!(parsed.custom_pricing.is_empty());
    }

    #[test]
    fn test_cost_tracker_with_custom_pricing() {
        let mut custom = HashMap::new();
        custom.insert(
            "my-llm".to_string(),
            ModelPricing {
                input_cost_per_million: 10.0,
                output_cost_per_million: 50.0,
            },
        );
        let tracker = CostTracker::new_with_pricing(custom);

        tracker.record("custom-provider", "my-llm", 1_000_000, 1_000_000);
        // 1M/1M * 10 + 1M/1M * 50 = 60.0
        assert!((tracker.total_cost() - 60.0).abs() < 1e-10);
    }
}
