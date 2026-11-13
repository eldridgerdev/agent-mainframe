//! Per-model rates, in USD per million tokens.

use crate::token_tracking::TokenPricingConfig;

/// Prompt size above which a long-prompt tier applies. The prompt is
/// input + cache read + cache write.
pub const LONG_PROMPT_THRESHOLD: u64 = 100_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LongPromptTier {
    /// The tier applies when the prompt has more than this many tokens.
    pub threshold_tokens: u64,
    /// Multiplies every rate of that call.
    pub multiplier: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelRates {
    pub input: f64,
    pub output: f64,
    pub reasoning: f64,
    pub cache_read: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub long_prompt: Option<LongPromptTier>,
    /// Cache and reasoning rates are assumed until confirmed: built-in rows
    /// derive them (reasoning = output, cache read = 10% of input, 5m write
    /// = 125%, 1h write = 200%). Also true for every fallback row.
    pub assumed: bool,
}

/// The 5-generation families listed in CLAUDE.md's model table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    Fable,
    Opus,
    Sonnet,
    Haiku,
}

impl ModelFamily {
    fn name(self) -> &'static str {
        match self {
            Self::Fable => "fable",
            Self::Opus => "opus",
            Self::Sonnet => "sonnet",
            Self::Haiku => "haiku",
        }
    }

    const ALL: [ModelFamily; 4] = [Self::Fable, Self::Opus, Self::Sonnet, Self::Haiku];

    /// Built-in rates, USD per MTok.
    pub fn rates(self) -> ModelRates {
        let (input, output) = match self {
            Self::Fable => (8.0, 40.0),
            Self::Opus => (4.0, 20.0),
            Self::Sonnet => (2.0, 10.0),
            Self::Haiku => (0.10, 0.50),
        };
        ModelRates {
            input,
            output,
            reasoning: output,
            cache_read: input * 0.10,
            cache_write_5m: input * 1.25,
            cache_write_1h: input * 2.0,
            long_prompt: matches!(self, Self::Haiku).then_some(LongPromptTier {
                threshold_tokens: LONG_PROMPT_THRESHOLD,
                multiplier: 5.0,
            }),
            assumed: true,
        }
    }
}

/// Rates for a model id, and whether the whole price is a guess.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PricedModel {
    /// None when the model fell back to `TokenPricingConfig`.
    pub family: Option<ModelFamily>,
    pub rates: ModelRates,
    /// True for a model with no built-in row; the ledger shows "price
    /// assumed".
    pub price_assumed: bool,
}

/// Maps an id such as `claude-opus-5-5`, `claude-haiku-5-5-20261001`,
/// `us.anthropic.claude-sonnet-5-5-v1:0`, `opus` or `claude-fable-5-2[1m]`
/// to a built-in family. Only generation 5 is built in, so
/// `claude-sonnet-4-5`, `claude-3-opus-20240229` and `gpt-5` give None.
pub fn family_for(model: &str) -> Option<ModelFamily> {
    let id = model.trim().to_ascii_lowercase();
    for family in ModelFamily::ALL {
        let Some(at) = id.find(family.name()) else {
            continue;
        };
        let rest = &id[at + family.name().len()..];
        let first = rest
            .trim_start_matches(['-', '.', '_'])
            .split(['-', '.', '_', ':', '['])
            .next()
            .unwrap_or("");
        // Unversioned (`opus`, `claude-opus`, `...-v1:0`) or generation 5.
        if first.is_empty() || first == "5" || first == "latest" || first.starts_with('v') {
            return Some(family);
        }
        return None;
    }
    None
}

/// Rates for `model`, falling back to the default `TokenPricingConfig`.
pub fn lookup(model: &str) -> PricedModel {
    lookup_with(model, &TokenPricingConfig::default())
}

/// Like [`lookup`], with the fallback config supplied by the caller.
pub fn lookup_with(model: &str, fallback: &TokenPricingConfig) -> PricedModel {
    match family_for(model) {
        Some(family) => PricedModel {
            family: Some(family),
            rates: family.rates(),
            price_assumed: false,
        },
        None => PricedModel {
            family: None,
            rates: ModelRates {
                input: fallback.input_per_mtok,
                output: fallback.output_per_mtok,
                reasoning: fallback.reasoning_per_mtok,
                cache_read: fallback.cache_read_per_mtok,
                // The config has one cache-write rate for both lifetimes.
                cache_write_5m: fallback.cache_write_per_mtok,
                cache_write_1h: fallback.cache_write_per_mtok,
                long_prompt: None,
                assumed: true,
            },
            price_assumed: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_model_ids() {
        for (id, family) in [
            ("claude-opus-5-5", ModelFamily::Opus),
            ("claude-fable-5-2", ModelFamily::Fable),
            ("claude-haiku-5-5", ModelFamily::Haiku),
            ("us.anthropic.claude-sonnet-5-5-v1:0", ModelFamily::Sonnet),
            ("claude-opus-5-5-20261001", ModelFamily::Opus),
            ("claude-fable-5-2[1m]", ModelFamily::Fable),
            ("Opus", ModelFamily::Opus),
            ("sonnet", ModelFamily::Sonnet),
            ("haiku", ModelFamily::Haiku),
            ("fable", ModelFamily::Fable),
        ] {
            assert_eq!(family_for(id), Some(family), "{id}");
            assert!(!lookup(id).price_assumed, "{id}");
        }
    }

    #[test]
    fn older_and_unknown_models_fall_back() {
        for id in [
            "claude-sonnet-4-5",
            "claude-3-opus-20240229",
            "claude-opus-4-20250514",
            "gpt-5",
            "",
        ] {
            let priced = lookup(id);
            assert_eq!(priced.family, None, "{id}");
            assert!(priced.price_assumed, "{id}");
            assert!(priced.rates.assumed);
        }
        let config = TokenPricingConfig::default();
        let priced = lookup("gpt-5");
        assert_eq!(priced.rates.input, config.input_per_mtok);
        assert_eq!(priced.rates.cache_write_5m, config.cache_write_per_mtok);
        assert_eq!(priced.rates.cache_write_1h, config.cache_write_per_mtok);
        assert_eq!(priced.rates.long_prompt, None);
    }

    #[test]
    fn builtin_rates_match_the_table() {
        let fable = lookup("fable").rates;
        assert_eq!(
            (fable.input, fable.output, fable.reasoning),
            (8.0, 40.0, 40.0)
        );
        assert_eq!(fable.cache_read, 0.8);
        assert_eq!(fable.cache_write_5m, 10.0);
        assert_eq!(fable.cache_write_1h, 16.0);
        assert!(fable.long_prompt.is_none());
        let haiku = lookup("haiku").rates;
        assert_eq!(haiku.long_prompt.unwrap().threshold_tokens, 100_000);
        assert_eq!(haiku.long_prompt.unwrap().multiplier, 5.0);
        assert_eq!(
            (lookup("opus").rates.input, lookup("opus").rates.output),
            (4.0, 20.0)
        );
        assert_eq!(
            (lookup("sonnet").rates.input, lookup("sonnet").rates.output),
            (2.0, 10.0)
        );
    }
}
