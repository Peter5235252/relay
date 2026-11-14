//! Token usage and list-price cost estimates.

use serde::{Deserialize, Serialize};

/// Token counts for one model response.
///
/// `input_tokens` is the *total* prompt size, including `cached_input_tokens`.
/// `reasoning_tokens` is a subset of `output_tokens` and is reported for
/// information only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Prompt tokens, including cached tokens.
    pub input_tokens: u32,
    /// Completion tokens, including reasoning tokens.
    pub output_tokens: u32,
    /// Prompt tokens served from a provider cache.
    pub cached_input_tokens: u32,
    /// Reasoning tokens reported by the provider.
    pub reasoning_tokens: u32,
}

impl Usage {
    /// Create a usage record.
    pub const fn new(
        input_tokens: u32,
        output_tokens: u32,
        cached_input_tokens: u32,
        reasoning_tokens: u32,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            reasoning_tokens,
        }
    }

    /// Total billed tokens (cached tokens are not double counted).
    pub const fn total_tokens(self) -> u64 {
        self.input_tokens as u64 + self.output_tokens as u64
    }

    /// Whether no usage information was received.
    pub const fn is_empty(self) -> bool {
        self.input_tokens == 0 && self.output_tokens == 0
    }

    /// Merge incremental usage events, preferring the largest values seen.
    pub fn merge_max(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.max(other.input_tokens);
        self.output_tokens = self.output_tokens.max(other.output_tokens);
        self.cached_input_tokens = self.cached_input_tokens.max(other.cached_input_tokens);
        self.reasoning_tokens = self.reasoning_tokens.max(other.reasoning_tokens);
    }
}

/// Prices for a model's long-context tier.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LongContextPricing {
    /// Input token count above which this tier applies.
    pub threshold_tokens: u32,
    /// Input price per million tokens.
    pub input_per_mtok: f64,
    /// Output price per million tokens.
    pub output_per_mtok: f64,
    /// Cached input price per million tokens, when published.
    pub cached_input_per_mtok: Option<f64>,
}

/// Per-token rates selected for a request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenRates {
    /// Input price per million tokens.
    pub input_per_mtok: f64,
    /// Output price per million tokens.
    pub output_per_mtok: f64,
    /// Cached input price per million tokens.
    pub cached_input_per_mtok: f64,
    /// Whether the long-context tier applied.
    pub long_context: bool,
}

/// A USD cost estimate broken down by token class.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostEstimate {
    /// Cost of uncached prompt tokens.
    pub input_usd: f64,
    /// Cost of cached prompt tokens.
    pub cached_usd: f64,
    /// Cost of completion tokens.
    pub output_usd: f64,
    /// Total cost.
    pub total_usd: f64,
}

impl CostEstimate {
    /// Format as a compact dollar amount suitable for a message footer.
    pub fn compact(&self) -> String {
        if self.total_usd <= 0.0 {
            "$0.00".to_owned()
        } else if self.total_usd < 0.01 {
            format!("${:.4}", self.total_usd)
        } else {
            format!("${:.2}", self.total_usd)
        }
    }
}

/// Public list pricing for one model, in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Pricing {
    /// Standard input price per million tokens.
    pub input_per_mtok: f64,
    /// Standard output price per million tokens.
    pub output_per_mtok: f64,
    /// Cached input price per million tokens, when published.
    pub cached_input_per_mtok: Option<f64>,
    /// Optional higher tier for long prompts.
    pub long_context: Option<LongContextPricing>,
    /// Public note such as introductory pricing.
    pub note: Option<&'static str>,
}

impl Pricing {
    /// Create standard pricing.
    pub const fn new(input_per_mtok: f64, output_per_mtok: f64) -> Self {
        Self {
            input_per_mtok,
            output_per_mtok,
            cached_input_per_mtok: None,
            long_context: None,
            note: None,
        }
    }

    /// Add a long-context tier.
    pub const fn with_long_context(
        mut self,
        threshold_tokens: u32,
        input_per_mtok: f64,
        output_per_mtok: f64,
    ) -> Self {
        self.long_context = Some(LongContextPricing {
            threshold_tokens,
            input_per_mtok,
            output_per_mtok,
            cached_input_per_mtok: None,
        });
        self
    }

    /// Add a cached-input price.
    pub const fn with_cached_input(mut self, cached_input_per_mtok: f64) -> Self {
        self.cached_input_per_mtok = Some(cached_input_per_mtok);
        self
    }

    /// Add a public pricing note.
    pub const fn with_note(mut self, note: &'static str) -> Self {
        self.note = Some(note);
        self
    }

    /// Select rates for the total prompt size.
    pub fn rates_for(&self, input_tokens: u32) -> TokenRates {
        if let Some(long) = self.long_context
            && input_tokens > long.threshold_tokens
        {
            return TokenRates {
                input_per_mtok: long.input_per_mtok,
                output_per_mtok: long.output_per_mtok,
                cached_input_per_mtok: long
                    .cached_input_per_mtok
                    .or(self.cached_input_per_mtok)
                    .unwrap_or(long.input_per_mtok),
                long_context: true,
            };
        }
        TokenRates {
            input_per_mtok: self.input_per_mtok,
            output_per_mtok: self.output_per_mtok,
            cached_input_per_mtok: self.cached_input_per_mtok.unwrap_or(self.input_per_mtok),
            long_context: false,
        }
    }

    /// Estimate the USD cost of a usage record.
    pub fn estimate(&self, usage: &Usage) -> f64 {
        self.breakdown(usage).total_usd
    }

    /// Estimate a USD cost with a breakdown by token class.
    pub fn breakdown(&self, usage: &Usage) -> CostEstimate {
        let rates = self.rates_for(usage.input_tokens);
        let cached = usage.cached_input_tokens.min(usage.input_tokens);
        let uncached = usage.input_tokens.saturating_sub(cached);
        let input_usd = uncached as f64 / 1_000_000.0 * rates.input_per_mtok;
        let cached_usd = cached as f64 / 1_000_000.0 * rates.cached_input_per_mtok;
        let output_usd = usage.output_tokens as f64 / 1_000_000.0 * rates.output_per_mtok;
        CostEstimate {
            input_usd,
            cached_usd,
            output_usd,
            total_usd: input_usd + cached_usd + output_usd,
        }
    }
}

/// Format a token count compactly (`1.2K`, `3.4M`).
pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_cost_is_calculated_per_million_tokens() {
        let pricing = Pricing::new(2.0, 10.0).with_cached_input(0.2);
        let usage = Usage::new(1_000_000, 100_000, 0, 0);
        let estimate = pricing.breakdown(&usage);
        assert!((estimate.input_usd - 2.0).abs() < f64::EPSILON);
        assert!((estimate.output_usd - 1.0).abs() < f64::EPSILON);
        assert!((estimate.total_usd - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cached_tokens_use_the_discounted_rate() {
        let pricing = Pricing::new(2.0, 10.0).with_cached_input(0.2);
        let usage = Usage::new(1_000_000, 0, 500_000, 0);
        let estimate = pricing.breakdown(&usage);
        assert!((estimate.input_usd - 1.0).abs() < f64::EPSILON);
        assert!((estimate.cached_usd - 0.1).abs() < f64::EPSILON);
        assert!((estimate.total_usd - 1.1).abs() < f64::EPSILON);
    }

    #[test]
    fn long_context_tier_applies_above_threshold() {
        let pricing = Pricing::new(2.0, 10.0).with_long_context(200_000, 4.0, 15.0);
        assert!(!pricing.rates_for(200_000).long_context);
        let rates = pricing.rates_for(200_001);
        assert!(rates.long_context);
        assert!((rates.input_per_mtok - 4.0).abs() < f64::EPSILON);
        assert!((rates.output_per_mtok - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn usage_merge_keeps_largest_values() {
        let mut usage = Usage::new(10, 5, 2, 0);
        usage.merge_max(Usage::new(8, 9, 0, 4));
        assert_eq!(usage, Usage::new(10, 9, 2, 4));
    }

    #[test]
    fn compact_cost_formatting() {
        assert_eq!(
            CostEstimate {
                input_usd: 0.0,
                cached_usd: 0.0,
                output_usd: 0.0,
                total_usd: 0.0
            }
            .compact(),
            "$0.00"
        );
        assert_eq!(
            CostEstimate {
                input_usd: 0.0,
                cached_usd: 0.0,
                output_usd: 0.0,
                total_usd: 0.0042
            }
            .compact(),
            "$0.0042"
        );
    }
}
