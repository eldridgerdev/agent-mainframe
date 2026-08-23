//! Harness-neutral context-window state.
//!
//! Collectors normalize provider-specific telemetry into these types before
//! application state or rendering sees it. Calculation, reset detection, and
//! stale-sample policy are deliberately implemented in later layers.

#![allow(dead_code)] // Introduced ahead of dashboard integration.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

/// Display band assigned to a normalized context snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextBand {
    #[default]
    Normal,
    Warning,
    Critical,
}

/// Whether the usage value came directly from the harness or includes an
/// AMF calculation/heuristic that must be marked as an estimate in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextProvenance {
    Direct,
    Estimated,
}

/// Whether the snapshot still represents the latest collection attempt.
///
/// `sampled_at` remains the time of the last valid value. `checked_at` moves
/// forward on a failed refresh, allowing stale data to retain its original age.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextFreshness {
    #[default]
    Fresh,
    Stale,
}

/// Why AMF started a new context-usage generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextResetReason {
    NewConversation,
    Cleared,
    Compaction,
    Summarization,
    TokenRollback,
}

/// The most recent reset observed for a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextResetEvent {
    pub reason: ContextResetReason,
    pub detected_at: DateTime<Utc>,
}

/// Reset-detection state carried with a snapshot.
///
/// `generation` starts at zero and increments once per accepted reset. It lets
/// consumers distinguish two snapshots with identical token counts on opposite
/// sides of a compaction. `conversation_id` is the harness identity used to
/// detect fresh conversations; it is optional because malformed or early
/// telemetry may not provide one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextResetMetadata {
    pub generation: u64,
    pub conversation_id: Option<String>,
    pub last_reset: Option<ContextResetEvent>,
}

/// Provider-neutral input to the shared calculation policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextUsageSample {
    pub used_tokens: u64,
    /// `None` means the collector found usage but could not establish the
    /// effective model limit. Zero is retained so policy can classify a
    /// malformed/invalid limit separately from a missing one.
    pub context_limit: Option<u64>,
    pub provenance: ContextProvenance,
    pub sampled_at: DateTime<Utc>,
    pub checked_at: DateTime<Utc>,
    pub reset: ContextResetMetadata,
}

/// Recoverable reasons a raw sample cannot become a normalized snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextCalculationError {
    MissingContextLimit,
    InvalidContextLimit,
}

/// A whole percentage whose value is always in the inclusive `0..=100` range.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContextPercentage(u8);

impl ContextPercentage {
    pub const MIN: Self = Self(0);
    pub const MAX: Self = Self(100);

    /// Construct a percentage while clamping provider or calculated values to
    /// the representable display range.
    pub const fn clamped(value: i64) -> Self {
        if value <= 0 {
            Self::MIN
        } else if value >= 100 {
            Self::MAX
        } else {
            Self(value as u8)
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// A valid, harness-neutral view of one agent session's current context use.
///
/// Missing or invalid context limits do not produce a snapshot. A non-zero
/// limit makes that invariant explicit, while `ContextPercentage` guarantees
/// that over-limit token reports can be retained without displaying more than
/// 100%. Snapshots are transient for now; deriving serde traits does not decide
/// whether they will eventually be persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionContextSnapshot {
    pub used_tokens: u64,
    pub context_limit: NonZeroU64,
    pub percentage: ContextPercentage,
    pub band: ContextBand,
    pub provenance: ContextProvenance,
    pub freshness: ContextFreshness,
    /// When the harness data represented by this snapshot was observed.
    pub sampled_at: DateTime<Utc>,
    /// Most recent time AMF attempted to refresh this snapshot.
    pub checked_at: DateTime<Utc>,
    pub reset: ContextResetMetadata,
}

pub const CONTEXT_WARNING_PERCENT: u8 = 70;
pub const CONTEXT_CRITICAL_PERCENT: u8 = 85;

/// Normalize one collector sample using the feature's shared display policy.
///
/// Percentages use integer floor division, so the warning and critical bands
/// begin only when actual usage reaches the exact 70% and 85% boundaries.
/// Arithmetic is widened to `u128` before multiplication and percentages are
/// clamped at 100 for over-limit provider reports.
pub fn calculate_context_snapshot(
    sample: ContextUsageSample,
) -> Result<SessionContextSnapshot, ContextCalculationError> {
    let limit = sample
        .context_limit
        .ok_or(ContextCalculationError::MissingContextLimit)?;
    let context_limit =
        NonZeroU64::new(limit).ok_or(ContextCalculationError::InvalidContextLimit)?;
    let raw_percentage = (u128::from(sample.used_tokens) * 100 / u128::from(limit)).min(100) as i64;
    let percentage = ContextPercentage::clamped(raw_percentage);
    let band = match percentage.get() {
        CONTEXT_CRITICAL_PERCENT..=100 => ContextBand::Critical,
        CONTEXT_WARNING_PERCENT..=84 => ContextBand::Warning,
        _ => ContextBand::Normal,
    };

    Ok(SessionContextSnapshot {
        used_tokens: sample.used_tokens,
        context_limit,
        percentage,
        band,
        provenance: sample.provenance,
        freshness: ContextFreshness::Fresh,
        sampled_at: sample.sampled_at,
        checked_at: sample.checked_at,
        reset: sample.reset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn timestamp(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0).single().unwrap()
    }

    #[test]
    fn context_percentage_clamps_to_the_display_range() {
        assert_eq!(ContextPercentage::clamped(-1), ContextPercentage::MIN);
        assert_eq!(ContextPercentage::clamped(0), ContextPercentage::MIN);
        assert_eq!(ContextPercentage::clamped(64).get(), 64);
        assert_eq!(ContextPercentage::clamped(100), ContextPercentage::MAX);
        assert_eq!(ContextPercentage::clamped(175), ContextPercentage::MAX);
    }

    #[test]
    fn snapshot_keeps_sample_and_check_times_distinct() {
        let sampled_at = timestamp(1_000);
        let checked_at = timestamp(1_005);
        let snapshot = SessionContextSnapshot {
            used_tokens: 64_000,
            context_limit: NonZeroU64::new(100_000).unwrap(),
            percentage: ContextPercentage::clamped(64),
            band: ContextBand::Normal,
            provenance: ContextProvenance::Direct,
            freshness: ContextFreshness::Stale,
            sampled_at,
            checked_at,
            reset: ContextResetMetadata::default(),
        };

        assert_eq!(snapshot.sampled_at, sampled_at);
        assert_eq!(snapshot.checked_at, checked_at);
        assert_eq!(snapshot.freshness, ContextFreshness::Stale);
    }

    #[test]
    fn reset_metadata_identifies_generation_conversation_and_reason() {
        let detected_at = timestamp(2_000);
        let metadata = ContextResetMetadata {
            generation: 2,
            conversation_id: Some("conversation-2".to_string()),
            last_reset: Some(ContextResetEvent {
                reason: ContextResetReason::Compaction,
                detected_at,
            }),
        };

        assert_eq!(metadata.generation, 2);
        assert_eq!(metadata.conversation_id.as_deref(), Some("conversation-2"));
        assert_eq!(
            metadata.last_reset.as_ref().map(|reset| reset.reason),
            Some(ContextResetReason::Compaction)
        );
    }

    #[test]
    fn context_types_have_stable_serialized_names() {
        assert_eq!(
            serde_json::to_string(&ContextProvenance::Estimated).unwrap(),
            "\"estimated\""
        );
        assert_eq!(
            serde_json::to_string(&ContextResetReason::TokenRollback).unwrap(),
            "\"token-rollback\""
        );
        assert_eq!(
            serde_json::to_string(&ContextPercentage::clamped(85)).unwrap(),
            "85"
        );
    }

    fn sample(used_tokens: u64, context_limit: Option<u64>) -> ContextUsageSample {
        let now = timestamp(3_000);
        ContextUsageSample {
            used_tokens,
            context_limit,
            provenance: ContextProvenance::Direct,
            sampled_at: now,
            checked_at: now,
            reset: ContextResetMetadata::default(),
        }
    }

    #[test]
    fn calculation_uses_exact_warning_and_critical_boundaries() {
        let cases = [
            (69_999, ContextBand::Normal, 69),
            (70_000, ContextBand::Warning, 70),
            (84_999, ContextBand::Warning, 84),
            (85_000, ContextBand::Critical, 85),
        ];

        for (used, expected_band, expected_percentage) in cases {
            let snapshot = calculate_context_snapshot(sample(used, Some(100_000))).unwrap();
            assert_eq!(snapshot.band, expected_band, "used tokens: {used}");
            assert_eq!(
                snapshot.percentage.get(),
                expected_percentage,
                "used tokens: {used}"
            );
        }
    }

    #[test]
    fn calculation_clamps_over_limit_usage_without_losing_raw_tokens() {
        let snapshot = calculate_context_snapshot(sample(u64::MAX, Some(100_000))).unwrap();

        assert_eq!(snapshot.used_tokens, u64::MAX);
        assert_eq!(snapshot.percentage, ContextPercentage::MAX);
        assert_eq!(snapshot.band, ContextBand::Critical);
    }

    #[test]
    fn calculation_rejects_missing_and_zero_limits_recoverably() {
        assert_eq!(
            calculate_context_snapshot(sample(10, None)),
            Err(ContextCalculationError::MissingContextLimit)
        );
        assert_eq!(
            calculate_context_snapshot(sample(10, Some(0))),
            Err(ContextCalculationError::InvalidContextLimit)
        );
    }

    #[test]
    fn calculation_preserves_explicit_estimate_provenance() {
        let mut input = sample(64_000, Some(100_000));
        input.provenance = ContextProvenance::Estimated;

        let snapshot = calculate_context_snapshot(input).unwrap();

        assert_eq!(snapshot.provenance, ContextProvenance::Estimated);
    }
}
