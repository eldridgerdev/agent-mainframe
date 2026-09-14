//! Harness-neutral context-window state.
//!
//! Collectors normalize provider-specific telemetry into these types before
//! application state or rendering sees it. Calculation, reset detection, and
//! stale-sample policy are deliberately implemented in later layers.

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

/// Transient context state for one AMF agent session.
///
/// Keeping this outside the persisted project model avoids turning an old
/// sample into apparently fresh telemetry after an AMF restart. The reset
/// metadata remains available while a post-compaction sample is pending even
/// though there is deliberately no percentage to render.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionContextState {
    pub snapshot: Option<SessionContextSnapshot>,
    pub reset: ContextResetMetadata,
    pub awaiting_post_reset: bool,
}

pub const DEFAULT_CONTEXT_WARNING_PERCENT: u8 = 70;
pub const DEFAULT_CONTEXT_CRITICAL_PERCENT: u8 = 85;
const ROLLBACK_MIN_WINDOW_PERCENT: u64 = 10;
const ROLLBACK_MAX_RETAINED_PERCENT: u64 = 75;
const ROLLBACK_MIN_TOKENS: u64 = 10_000;

/// User-adjustable percentage boundaries for [`ContextBand`], threaded through
/// from [`crate::app::AppConfig`] so a customized value reaches the shared
/// calculation policy without every caller needing to know about `AppConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextThresholds {
    pub warning_percent: u8,
    pub critical_percent: u8,
}

impl Default for ContextThresholds {
    fn default() -> Self {
        Self {
            warning_percent: DEFAULT_CONTEXT_WARNING_PERCENT,
            critical_percent: DEFAULT_CONTEXT_CRITICAL_PERCENT,
        }
    }
}

/// Normalize one collector sample using the feature's shared display policy.
///
/// Percentages use integer floor division, so the warning and critical bands
/// begin only when actual usage reaches `thresholds`' exact boundaries.
/// Arithmetic is widened to `u128` before multiplication and percentages are
/// clamped at 100 for over-limit provider reports.
pub fn calculate_context_snapshot(
    sample: ContextUsageSample,
    thresholds: ContextThresholds,
) -> Result<SessionContextSnapshot, ContextCalculationError> {
    let limit = sample
        .context_limit
        .ok_or(ContextCalculationError::MissingContextLimit)?;
    let context_limit =
        NonZeroU64::new(limit).ok_or(ContextCalculationError::InvalidContextLimit)?;
    let raw_percentage = (u128::from(sample.used_tokens) * 100 / u128::from(limit)).min(100) as i64;
    let percentage = ContextPercentage::clamped(raw_percentage);
    let band = if percentage.get() >= thresholds.critical_percent {
        ContextBand::Critical
    } else if percentage.get() >= thresholds.warning_percent {
        ContextBand::Warning
    } else {
        ContextBand::Normal
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

impl SessionContextState {
    /// Accept a valid collector sample, applying lifecycle and conservative
    /// rollback detection before publishing its percentage.
    pub fn accept_sample(
        &mut self,
        mut sample: ContextUsageSample,
        thresholds: ContextThresholds,
    ) -> Result<(), ContextCalculationError> {
        let previous = self.snapshot.as_ref();
        let conversation_changed = self
            .reset
            .conversation_id
            .as_deref()
            .zip(sample.reset.conversation_id.as_deref())
            .is_some_and(|(before, after)| before != after);
        let explicit_reset = sample
            .reset
            .last_reset
            .as_ref()
            .filter(|event| self.reset.last_reset.as_ref() != Some(*event));
        let rollback = previous.is_some_and(|previous| token_rollback(previous, &sample));

        let detected_reset = if conversation_changed {
            Some(ContextResetEvent {
                reason: ContextResetReason::NewConversation,
                detected_at: sample.sampled_at,
            })
        } else if let Some(event) = explicit_reset {
            Some(event.clone())
        } else if rollback {
            Some(ContextResetEvent {
                reason: ContextResetReason::TokenRollback,
                detected_at: sample.sampled_at,
            })
        } else {
            None
        };

        let had_prior_measurement = previous.is_some() || self.awaiting_post_reset;
        if detected_reset.is_some() && had_prior_measurement && !self.awaiting_post_reset {
            self.reset.generation = self.reset.generation.saturating_add(1);
        }
        if let Some(event) = detected_reset.or(sample.reset.last_reset.take()) {
            self.reset.last_reset = Some(event);
        }
        if sample.reset.conversation_id.is_some() {
            self.reset.conversation_id = sample.reset.conversation_id.take();
        }

        sample.reset = self.reset.clone();
        let snapshot = calculate_context_snapshot(sample, thresholds)?;
        self.snapshot = Some(snapshot);
        self.awaiting_post_reset = false;
        Ok(())
    }

    /// Retain the last valid value while making it explicit that the latest
    /// refresh failed. Its original `sampled_at` remains unchanged.
    pub fn mark_unavailable(&mut self, checked_at: DateTime<Utc>) {
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.freshness = ContextFreshness::Stale;
            snapshot.checked_at = checked_at;
        }
    }

    /// Clear the displayed percentage after an explicit reset. Repeated
    /// pending observations belong to the same generation.
    pub fn begin_reset(&mut self, conversation_id: Option<String>, event: ContextResetEvent) {
        let conversation_changed = self
            .reset
            .conversation_id
            .as_deref()
            .zip(conversation_id.as_deref())
            .is_some_and(|(before, after)| before != after);
        if !self.awaiting_post_reset && self.snapshot.is_some() {
            self.reset.generation = self.reset.generation.saturating_add(1);
        }
        self.reset.last_reset = Some(if conversation_changed {
            ContextResetEvent {
                reason: ContextResetReason::NewConversation,
                detected_at: event.detected_at,
            }
        } else {
            event
        });
        if conversation_id.is_some() {
            self.reset.conversation_id = conversation_id;
        }
        self.snapshot = None;
        self.awaiting_post_reset = true;
    }
}

fn token_rollback(previous: &SessionContextSnapshot, sample: &ContextUsageSample) -> bool {
    if sample.used_tokens >= previous.used_tokens {
        return false;
    }
    let limit = sample.context_limit.unwrap_or(previous.context_limit.get());
    let minimum_drop = (limit / 100)
        .saturating_mul(ROLLBACK_MIN_WINDOW_PERCENT)
        .max(ROLLBACK_MIN_TOKENS);
    let drop = previous.used_tokens.saturating_sub(sample.used_tokens);
    let retained_enough_to_be_correction = u128::from(sample.used_tokens) * 100
        > u128::from(previous.used_tokens) * u128::from(ROLLBACK_MAX_RETAINED_PERCENT);

    drop >= minimum_drop && !retained_enough_to_be_correction
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
            let snapshot = calculate_context_snapshot(
                sample(used, Some(100_000)),
                ContextThresholds::default(),
            )
            .unwrap();
            assert_eq!(snapshot.band, expected_band, "used tokens: {used}");
            assert_eq!(
                snapshot.percentage.get(),
                expected_percentage,
                "used tokens: {used}"
            );
        }
    }

    #[test]
    fn custom_thresholds_move_the_severity_boundaries() {
        let thresholds = ContextThresholds {
            warning_percent: 50,
            critical_percent: 90,
        };

        // 60% clears the default 70% warning boundary but not the customized
        // 50% one.
        let mid = calculate_context_snapshot(sample(60_000, Some(100_000)), thresholds).unwrap();
        assert_eq!(mid.band, ContextBand::Warning);

        // 85% trips the default 85% critical boundary but stays Warning
        // under the customized 90% one.
        let high = calculate_context_snapshot(sample(85_000, Some(100_000)), thresholds).unwrap();
        assert_eq!(high.band, ContextBand::Warning);

        let critical =
            calculate_context_snapshot(sample(90_000, Some(100_000)), thresholds).unwrap();
        assert_eq!(critical.band, ContextBand::Critical);
    }

    #[test]
    fn custom_context_window_changes_the_percentage_and_band() {
        // Same raw usage, a larger (customized) context window: the
        // percentage — and therefore the band — must be computed against the
        // override, not some other fixed limit.
        let small_window =
            calculate_context_snapshot(sample(70_000, Some(100_000)), ContextThresholds::default())
                .unwrap();
        assert_eq!(small_window.percentage.get(), 70);
        assert_eq!(small_window.band, ContextBand::Warning);

        let large_window = calculate_context_snapshot(
            sample(70_000, Some(1_000_000)),
            ContextThresholds::default(),
        )
        .unwrap();
        assert_eq!(large_window.percentage.get(), 7);
        assert_eq!(large_window.band, ContextBand::Normal);
    }

    #[test]
    fn calculation_clamps_over_limit_usage_without_losing_raw_tokens() {
        let snapshot = calculate_context_snapshot(
            sample(u64::MAX, Some(100_000)),
            ContextThresholds::default(),
        )
        .unwrap();

        assert_eq!(snapshot.used_tokens, u64::MAX);
        assert_eq!(snapshot.percentage, ContextPercentage::MAX);
        assert_eq!(snapshot.band, ContextBand::Critical);
    }

    #[test]
    fn calculation_rejects_missing_and_zero_limits_recoverably() {
        assert_eq!(
            calculate_context_snapshot(sample(10, None), ContextThresholds::default()),
            Err(ContextCalculationError::MissingContextLimit)
        );
        assert_eq!(
            calculate_context_snapshot(sample(10, Some(0)), ContextThresholds::default()),
            Err(ContextCalculationError::InvalidContextLimit)
        );
    }

    #[test]
    fn calculation_preserves_explicit_estimate_provenance() {
        let mut input = sample(64_000, Some(100_000));
        input.provenance = ContextProvenance::Estimated;

        let snapshot = calculate_context_snapshot(input, ContextThresholds::default()).unwrap();

        assert_eq!(snapshot.provenance, ContextProvenance::Estimated);
    }

    #[test]
    fn unavailable_refresh_retains_the_previous_sample_as_stale() {
        let mut state = SessionContextState::default();
        let input = sample(64_000, Some(100_000));
        let sampled_at = input.sampled_at;
        state
            .accept_sample(input, ContextThresholds::default())
            .unwrap();

        state.mark_unavailable(timestamp(3_005));

        let snapshot = state.snapshot.unwrap();
        assert_eq!(snapshot.freshness, ContextFreshness::Stale);
        assert_eq!(snapshot.sampled_at, sampled_at);
        assert_eq!(snapshot.checked_at, timestamp(3_005));
    }

    #[test]
    fn changed_conversation_starts_one_new_generation() {
        let mut state = SessionContextState::default();
        let mut first = sample(80_000, Some(100_000));
        first.reset.conversation_id = Some("conversation-1".to_string());
        state
            .accept_sample(first, ContextThresholds::default())
            .unwrap();

        let mut second = sample(1_000, Some(100_000));
        second.sampled_at = timestamp(3_010);
        second.reset.conversation_id = Some("conversation-2".to_string());
        state
            .accept_sample(second, ContextThresholds::default())
            .unwrap();

        assert_eq!(state.reset.generation, 1);
        assert_eq!(
            state.reset.last_reset.as_ref().map(|event| event.reason),
            Some(ContextResetReason::NewConversation)
        );
        assert_eq!(state.snapshot.unwrap().band, ContextBand::Normal);
    }

    #[test]
    fn pending_compaction_clears_usage_until_the_next_sample() {
        let mut state = SessionContextState::default();
        let mut first = sample(90_000, Some(100_000));
        first.reset.conversation_id = Some("conversation-1".to_string());
        state
            .accept_sample(first, ContextThresholds::default())
            .unwrap();
        let event = ContextResetEvent {
            reason: ContextResetReason::Compaction,
            detected_at: timestamp(3_010),
        };

        state.begin_reset(Some("conversation-1".to_string()), event.clone());
        state.begin_reset(Some("conversation-1".to_string()), event.clone());

        assert!(state.snapshot.is_none());
        assert!(state.awaiting_post_reset);
        assert_eq!(state.reset.generation, 1);

        let mut after = sample(20_000, Some(100_000));
        after.sampled_at = timestamp(3_020);
        after.reset.conversation_id = Some("conversation-1".to_string());
        after.reset.last_reset = Some(event);
        state
            .accept_sample(after, ContextThresholds::default())
            .unwrap();

        assert_eq!(state.reset.generation, 1);
        assert!(!state.awaiting_post_reset);
        assert_eq!(state.snapshot.unwrap().band, ContextBand::Normal);
    }

    #[test]
    fn rollback_detection_ignores_corrections_but_accepts_large_drops() {
        let mut state = SessionContextState::default();
        state
            .accept_sample(sample(80_000, Some(100_000)), ContextThresholds::default())
            .unwrap();

        state
            .accept_sample(sample(78_000, Some(100_000)), ContextThresholds::default())
            .unwrap();
        assert_eq!(state.reset.generation, 0);

        state
            .accept_sample(sample(40_000, Some(100_000)), ContextThresholds::default())
            .unwrap();
        assert_eq!(state.reset.generation, 1);
        assert_eq!(
            state.reset.last_reset.as_ref().map(|event| event.reason),
            Some(ContextResetReason::TokenRollback)
        );
    }
}
