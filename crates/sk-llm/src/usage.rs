//! Token usage, model prices and cost estimates (SPEC-08 §4.2.3, NFR-08-03).

use serde::{Deserialize, Serialize};

use crate::ANTHROPIC_PROVIDER;

/// Estimated input tokens per classified item, system prompt share included (NFR-08-03).
pub const EST_INPUT_TOKENS_PER_ITEM: u64 = 600;
/// Estimated output tokens per classified item (NFR-08-03).
pub const EST_OUTPUT_TOKENS_PER_ITEM: u64 = 120;

/// Tokens a provider reported and what they cost.
///
/// Local providers report nothing (all zero). The counters of a classifier
/// are cumulative since it was created ([`Classifier::usage`](crate::Classifier::usage)).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct UsageStats {
    /// Successful (2xx) classification requests.
    pub requests: u64,
    /// Uncached input tokens (`usage.input_tokens`).
    pub input_tokens: u64,
    /// Output tokens (`usage.output_tokens`).
    pub output_tokens: u64,
    /// Input tokens written to the prompt cache (`usage.cache_creation_input_tokens`).
    pub cache_creation_input_tokens: u64,
    /// Input tokens read from the prompt cache (`usage.cache_read_input_tokens`).
    pub cache_read_input_tokens: u64,
    /// Actual cost in USD by [`ModelPricing`]; 0 when the model's price is unknown.
    pub cost_usd: f64,
}

impl UsageStats {
    /// Adds `other` to `self` (saturating counters).
    pub fn add(&mut self, other: &UsageStats) {
        self.requests = self.requests.saturating_add(other.requests);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(other.cache_creation_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(other.cache_read_input_tokens);
        self.cost_usd += other.cost_usd;
    }
}

/// Price of a model in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ModelPricing {
    /// Uncached input.
    pub input_per_mtok: f64,
    /// Output.
    pub output_per_mtok: f64,
    /// Prompt cache write (5-minute TTL, 1.25 × input).
    pub cache_write_per_mtok: f64,
    /// Prompt cache read (0.1 × input).
    pub cache_read_per_mtok: f64,
}

impl ModelPricing {
    /// The price of `model` at `provider`; `None` for local providers and
    /// unknown models. Model ids match exactly (case-insensitive, trimmed).
    pub fn for_model(provider: &str, model: &str) -> Option<Self> {
        if provider != ANTHROPIC_PROVIDER {
            return None;
        }
        let (input, output) = match model.trim().to_lowercase().as_str() {
            "claude-haiku-4-5" => (1.0, 5.0),
            "claude-sonnet-5" => (2.0, 10.0),
            _ => return None,
        };
        Some(Self {
            input_per_mtok: input,
            output_per_mtok: output,
            cache_write_per_mtok: input * 1.25,
            cache_read_per_mtok: input * 0.1,
        })
    }

    /// Cost in USD of the token counts in `usage` (its `cost_usd` is ignored).
    pub fn cost_usd(&self, usage: &UsageStats) -> f64 {
        let mtok = |n: u64| n as f64 / 1_000_000.0;
        mtok(usage.input_tokens) * self.input_per_mtok
            + mtok(usage.output_tokens) * self.output_per_mtok
            + mtok(usage.cache_creation_input_tokens) * self.cache_write_per_mtok
            + mtok(usage.cache_read_input_tokens) * self.cache_read_per_mtok
    }
}

/// Cost estimate shown before a cloud classification (§4.8, NFR-08-03).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CostEstimate {
    /// Items to classify.
    pub items: usize,
    /// `items × EST_INPUT_TOKENS_PER_ITEM`.
    pub input_tokens: u64,
    /// `items × EST_OUTPUT_TOKENS_PER_ITEM`.
    pub output_tokens: u64,
    /// Estimated cost in USD (no prompt-cache discount).
    pub usd: f64,
}

impl CostEstimate {
    /// Estimate for `n_items` at `pricing`.
    pub fn new(pricing: &ModelPricing, n_items: usize) -> Self {
        let n = u64::try_from(n_items).unwrap_or(u64::MAX);
        let input_tokens = n.saturating_mul(EST_INPUT_TOKENS_PER_ITEM);
        let output_tokens = n.saturating_mul(EST_OUTPUT_TOKENS_PER_ITEM);
        let usd = pricing.cost_usd(&UsageStats {
            input_tokens,
            output_tokens,
            ..UsageStats::default()
        });
        Self {
            items: n_items,
            input_tokens,
            output_tokens,
            usd,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn prices_of_the_default_models() {
        let haiku = ModelPricing::for_model("anthropic", "claude-haiku-4-5");
        let Some(haiku) = haiku else {
            panic!("haiku price missing")
        };
        assert!(close(haiku.input_per_mtok, 1.0));
        assert!(close(haiku.output_per_mtok, 5.0));
        assert!(close(haiku.cache_write_per_mtok, 1.25));
        assert!(close(haiku.cache_read_per_mtok, 0.1));
        assert!(ModelPricing::for_model("anthropic", " Claude-Sonnet-5 ").is_some());
        assert!(ModelPricing::for_model("anthropic", "claude-haiku-4-5-20251001").is_none());
        assert!(ModelPricing::for_model("ollama", "claude-haiku-4-5").is_none());
    }

    #[test]
    fn cost_counts_every_token_kind() {
        let Some(p) = ModelPricing::for_model("anthropic", "claude-haiku-4-5") else {
            panic!("haiku price missing")
        };
        let usage = UsageStats {
            requests: 1,
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_creation_input_tokens: 1_000_000,
            cache_read_input_tokens: 1_000_000,
            cost_usd: 99.0,
        };
        assert!(close(p.cost_usd(&usage), 1.0 + 0.5 + 1.25 + 0.1));
    }

    #[test]
    fn estimate_for_a_full_scan() {
        let Some(p) = ModelPricing::for_model("anthropic", "claude-haiku-4-5") else {
            panic!("haiku price missing")
        };
        let e = CostEstimate::new(&p, 300);
        assert_eq!(
            (e.items, e.input_tokens, e.output_tokens),
            (300, 180_000, 36_000)
        );
        assert!(close(e.usd, 0.18 + 0.18));
    }

    #[test]
    fn usage_adds_up() {
        let mut total = UsageStats::default();
        let one = UsageStats {
            requests: 1,
            input_tokens: 10,
            output_tokens: 2,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 4,
            cost_usd: 0.5,
        };
        total.add(&one);
        total.add(&one);
        assert_eq!(total.requests, 2);
        assert_eq!(total.input_tokens, 20);
        assert_eq!(total.cache_read_input_tokens, 8);
        assert!(close(total.cost_usd, 1.0));
    }
}
