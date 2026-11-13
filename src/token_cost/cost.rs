//! Pricing one call from its token counts.

use super::pricing::ModelRates;

/// Token counts for one call. `None` means unavailable, not zero.
///
/// Claude's rule, which this module relies on: `input_tokens` excludes cache
/// reads and cache creation, so `input`, `cache_read` and the cache writes
/// are additive and disjoint. No token is counted twice. Claude's reasoning
/// tokens are already inside `output`, so Claude rows leave `reasoning`
/// `None`; it is priced additively only for a provider that reports it
/// separately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenCounts {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    /// Cache writes with an unknown lifetime belong here.
    pub cache_write_5m: Option<u64>,
    pub cache_write_1h: Option<u64>,
    pub reasoning: Option<u64>,
}

fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        _ => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
    }
}

impl TokenCounts {
    fn all(&self) -> [Option<u64>; 6] {
        [
            self.input,
            self.output,
            self.cache_read,
            self.cache_write_5m,
            self.cache_write_1h,
            self.reasoning,
        ]
    }

    pub fn is_all_unavailable(&self) -> bool {
        self.all().iter().all(Option::is_none)
    }

    /// Input + cache read + cache writes, the size that selects a
    /// long-prompt tier.
    pub fn prompt_tokens(&self) -> u64 {
        [
            self.input,
            self.cache_read,
            self.cache_write_5m,
            self.cache_write_1h,
        ]
        .iter()
        .map(|v| v.unwrap_or(0))
        .fold(0, u64::saturating_add)
    }

    /// Field-wise sum; a field stays `None` only when both sides are.
    pub fn plus(&self, other: &TokenCounts) -> TokenCounts {
        TokenCounts {
            input: add(self.input, other.input),
            output: add(self.output, other.output),
            cache_read: add(self.cache_read, other.cache_read),
            cache_write_5m: add(self.cache_write_5m, other.cache_write_5m),
            cache_write_1h: add(self.cache_write_1h, other.cache_write_1h),
            reasoning: add(self.reasoning, other.reasoning),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Priced {
    pub cost_usd: f64,
    /// `cost / base input rate`, where the base rate is before any
    /// long-prompt multiplier, so a tiered call shows its multiplier here.
    /// None when every count is unavailable or the base input rate is zero.
    pub effective_tokens: Option<u64>,
    /// The rates used, after any long-prompt multiplier.
    pub rates_applied: ModelRates,
    pub long_prompt_applied: bool,
}

/// Prices `counts` at `rates`. Unavailable counts cost nothing. A long-prompt
/// tier multiplies every rate of the call.
pub fn price(counts: &TokenCounts, rates: &ModelRates) -> Priced {
    let mut applied = *rates;
    let mut long_prompt_applied = false;
    if let Some(tier) = rates.long_prompt
        && counts.prompt_tokens() > tier.threshold_tokens
    {
        for rate in [
            &mut applied.input,
            &mut applied.output,
            &mut applied.reasoning,
            &mut applied.cache_read,
            &mut applied.cache_write_5m,
            &mut applied.cache_write_1h,
        ] {
            *rate *= tier.multiplier;
        }
        long_prompt_applied = true;
    }

    let n = |v: Option<u64>| v.unwrap_or(0) as f64;
    let cost_usd = (n(counts.input) * applied.input
        + n(counts.output) * applied.output
        + n(counts.reasoning) * applied.reasoning
        + n(counts.cache_read) * applied.cache_read
        + n(counts.cache_write_5m) * applied.cache_write_5m
        + n(counts.cache_write_1h) * applied.cache_write_1h)
        / 1_000_000.0;

    let effective_tokens = (!counts.is_all_unavailable() && rates.input > 0.0)
        .then(|| (cost_usd * 1_000_000.0 / rates.input).round() as u64);

    Priced {
        cost_usd,
        effective_tokens,
        rates_applied: applied,
        long_prompt_applied,
    }
}

#[cfg(test)]
mod tests {
    use super::super::pricing::lookup;
    use super::*;

    fn counts(input: u64, output: u64) -> TokenCounts {
        TokenCounts {
            input: Some(input),
            output: Some(output),
            ..Default::default()
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn cost_per_family() {
        // 1M input + 1M output.
        for (model, expected) in [("fable", 48.0), ("opus", 24.0), ("sonnet", 12.0)] {
            let p = price(&counts(1_000_000, 1_000_000), &lookup(model).rates);
            assert!(approx(p.cost_usd, expected), "{model}: {}", p.cost_usd);
            assert!(!p.long_prompt_applied);
        }
        // Haiku's prompt is over the threshold, so the 5x tier applies.
        let p = price(&counts(1_000_000, 1_000_000), &lookup("haiku").rates);
        assert!(approx(p.cost_usd, 3.0));
        let p = price(&counts(50_000, 1_000_000), &lookup("haiku").rates);
        assert!(approx(p.cost_usd, 0.005 + 0.5));
    }

    #[test]
    fn haiku_long_prompt_tier_counts_cache_in_prompt() {
        let rates = lookup("haiku").rates;
        let below = TokenCounts {
            input: Some(40_000),
            cache_read: Some(30_000),
            cache_write_5m: Some(30_000),
            output: Some(1_000),
            ..Default::default()
        };
        assert!(!price(&below, &rates).long_prompt_applied);
        let above = TokenCounts {
            cache_read: Some(30_001),
            ..below
        };
        let p = price(&above, &rates);
        assert!(p.long_prompt_applied);
        assert!(approx(p.rates_applied.input, 0.5));
        assert!(approx(p.rates_applied.output, 2.5));
        assert!(approx(p.rates_applied.cache_read, 0.05));
        // The original rates are untouched.
        assert!(approx(rates.input, 0.10));
    }

    #[test]
    fn effective_tokens_use_the_input_rate() {
        let p = price(&counts(0, 1_000), &lookup("fable").rates);
        assert_eq!(p.effective_tokens, Some(5_000));
        let p = price(&counts(1_000, 0), &lookup("opus").rates);
        assert_eq!(p.effective_tokens, Some(1_000));
    }

    #[test]
    fn effective_tokens_show_the_long_prompt_multiplier() {
        let rates = lookup("haiku").rates;
        let short = price(&counts(1_000, 1_000), &rates);
        let long = price(&counts(100_001, 0), &rates);
        // Base input rate 0.10: 100_001 input at 5x is 500_005 effective.
        assert_eq!(long.effective_tokens, Some(500_005));
        assert_eq!(short.effective_tokens, Some(6_000));
    }

    #[test]
    fn cache_lifetimes_priced_separately() {
        let c = TokenCounts {
            cache_write_5m: Some(1_000_000),
            cache_write_1h: Some(1_000_000),
            cache_read: Some(1_000_000),
            ..Default::default()
        };
        let p = price(&c, &lookup("opus").rates);
        assert!(approx(p.cost_usd, 5.0 + 8.0 + 0.4));
    }

    #[test]
    fn unavailable_counts_are_not_zero() {
        let p = price(&TokenCounts::default(), &lookup("opus").rates);
        assert_eq!(p.cost_usd, 0.0);
        assert_eq!(p.effective_tokens, None);
        // A real zero is available.
        let p = price(&counts(0, 0), &lookup("opus").rates);
        assert_eq!(p.effective_tokens, Some(0));
        // Partial counts price what exists.
        let c = TokenCounts {
            output: Some(1_000_000),
            ..Default::default()
        };
        assert!(approx(price(&c, &lookup("opus").rates).cost_usd, 20.0));
        assert_eq!(c.plus(&TokenCounts::default()).input, None);
        assert_eq!(c.plus(&counts(1, 1)).input, Some(1));
    }

    #[test]
    fn unknown_model_prices_at_fallback() {
        let p = price(&counts(1_000_000, 0), &lookup("gpt-5").rates);
        assert!(approx(p.cost_usd, 3.0));
    }
}
