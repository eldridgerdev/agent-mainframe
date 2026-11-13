//! One ledger row per model call (plan section 7). Pure data and pricing;
//! storage comes later.

use chrono::{DateTime, Utc};

use super::cost::{TokenCounts, price};
use super::pricing::{ModelRates, lookup_with};
use crate::token_tracking::TokenPricingConfig;

macro_rules! kebab_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }

            pub fn parse(text: &str) -> Option<Self> {
                match text { $($text => Some($name::$variant),)+ _ => None }
            }
        }
    };
}

kebab_enum!(Phase {
    Capture => "capture",
    Prepare => "prepare",
    Discuss => "discuss",
    Refine => "refine",
});

kebab_enum!(Task {
    Brief => "brief",
    Architecture => "architecture",
    KeyFlow => "key-flow",
    Fit => "fit",
    Unexplained => "unexplained",
    PlanDrift => "plan-drift",
    QuestionL1 => "question-L1",
    QuestionL2 => "question-L2",
    QuestionL3 => "question-L3",
    Refinement => "refinement",
    DecisionRecord => "decision-record",
    InstructionCarry => "instruction-carry",
});

kebab_enum!(Agent {
    Developer => "developer",
    Scribe => "scribe",
    Cartographer => "cartographer",
    Presenter => "presenter",
});

kebab_enum!(Provenance {
    Measured => "measured",
    Estimated => "estimated",
});

/// Fallback when a call reports no usage: ceil(bytes / 4). Rows built from it
/// are `Provenance::Estimated`.
pub fn estimate_tokens_from_bytes(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
}

/// What a call was for.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerContext {
    pub phase: Phase,
    pub task: Task,
    pub agent: Agent,
    pub harness: String,
    pub model: String,
    pub reasoning: String,
    pub round: u32,
    /// None for packet-wide work.
    pub agenda_item: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LedgerRow {
    pub phase: Phase,
    pub task: Task,
    pub agent: Agent,
    pub harness: String,
    pub model: String,
    pub reasoning: String,
    pub round: u32,
    pub agenda_item: Option<String>,
    pub counts: TokenCounts,
    pub effective_tokens: Option<u64>,
    pub cost_usd: f64,
    /// Copied onto the row so a later price change does not rewrite history.
    pub rates_applied: ModelRates,
    pub provenance: Provenance,
    pub price_assumed: bool,
    pub cache_hit: bool,
    pub saved_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub started_at: DateTime<Utc>,
}

impl LedgerRow {
    /// Prices `counts` for `ctx.model`; unknown models use `fallback` and set
    /// `price_assumed`.
    pub fn from_counts(
        ctx: LedgerContext,
        counts: TokenCounts,
        provenance: Provenance,
        duration_ms: Option<u64>,
        started_at: DateTime<Utc>,
        fallback: &TokenPricingConfig,
    ) -> Self {
        let model = lookup_with(&ctx.model, fallback);
        let priced = price(&counts, &model.rates);
        Self {
            phase: ctx.phase,
            task: ctx.task,
            agent: ctx.agent,
            harness: ctx.harness,
            model: ctx.model,
            reasoning: ctx.reasoning,
            round: ctx.round,
            agenda_item: ctx.agenda_item,
            counts,
            effective_tokens: priced.effective_tokens,
            cost_usd: priced.cost_usd,
            rates_applied: priced.rates_applied,
            provenance,
            price_assumed: model.price_assumed,
            cache_hit: false,
            saved_usd: None,
            duration_ms,
            started_at,
        }
    }

    /// A reused cached artifact: zero spend, and what it saved.
    pub fn cache_hit(
        ctx: LedgerContext,
        saved_usd: f64,
        started_at: DateTime<Utc>,
        fallback: &TokenPricingConfig,
    ) -> Self {
        let mut row = Self::from_counts(
            ctx,
            TokenCounts::default(),
            Provenance::Measured,
            None,
            started_at,
            fallback,
        );
        row.cache_hit = true;
        row.saved_usd = Some(saved_usd);
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(model: &str) -> LedgerContext {
        LedgerContext {
            phase: Phase::Prepare,
            task: Task::KeyFlow,
            agent: Agent::Cartographer,
            harness: "claude".into(),
            model: model.into(),
            reasoning: "low".into(),
            round: 1,
            agenda_item: Some("item-1".into()),
        }
    }

    #[test]
    fn enum_strings_round_trip() {
        for p in Phase::ALL {
            assert_eq!(Phase::parse(p.as_str()), Some(*p));
        }
        for t in Task::ALL {
            assert_eq!(Task::parse(t.as_str()), Some(*t));
        }
        for a in Agent::ALL {
            assert_eq!(Agent::parse(a.as_str()), Some(*a));
        }
        for v in Provenance::ALL {
            assert_eq!(Provenance::parse(v.as_str()), Some(*v));
        }
        assert_eq!(Task::KeyFlow.as_str(), "key-flow");
        assert_eq!(Task::QuestionL2.as_str(), "question-L2");
        assert_eq!(Task::InstructionCarry.as_str(), "instruction-carry");
        assert_eq!(Task::parse("nope"), None);
    }

    #[test]
    fn bytes_estimate_rounds_up() {
        assert_eq!(estimate_tokens_from_bytes(0), 0);
        assert_eq!(estimate_tokens_from_bytes(1), 1);
        assert_eq!(estimate_tokens_from_bytes(4), 1);
        assert_eq!(estimate_tokens_from_bytes(5), 2);
    }

    #[test]
    fn row_is_priced_from_its_model() {
        let counts = TokenCounts {
            output: Some(1_000),
            ..Default::default()
        };
        let row = LedgerRow::from_counts(
            ctx("claude-fable-5-2"),
            counts,
            Provenance::Measured,
            Some(1200),
            Utc::now(),
            &TokenPricingConfig::default(),
        );
        assert_eq!(row.effective_tokens, Some(5_000));
        assert!((row.cost_usd - 0.04).abs() < 1e-9);
        assert_eq!(row.rates_applied.input, 8.0);
        assert!(!row.price_assumed);
        assert!(!row.cache_hit);
    }

    #[test]
    fn unknown_model_row_is_flagged() {
        let row = LedgerRow::from_counts(
            ctx("gpt-5"),
            TokenCounts {
                input: Some(1_000_000),
                ..Default::default()
            },
            Provenance::Estimated,
            None,
            Utc::now(),
            &TokenPricingConfig::default(),
        );
        assert!(row.price_assumed);
        assert!((row.cost_usd - 3.0).abs() < 1e-9);
    }

    #[test]
    fn cache_hit_row_spends_nothing() {
        let row = LedgerRow::cache_hit(
            ctx("opus"),
            0.12,
            Utc::now(),
            &TokenPricingConfig::default(),
        );
        assert!(row.cache_hit);
        assert_eq!(row.saved_usd, Some(0.12));
        assert_eq!(row.cost_usd, 0.0);
        assert_eq!(row.effective_tokens, None);
        assert!(row.counts.is_all_unavailable());
    }
}
