//! Layout-independent formatting for context-window indicators.

use crate::context_tracking::{
    ContextBand, ContextFreshness, ContextProvenance, SessionContextSnapshot,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextIndicator {
    pub text: String,
    pub band: ContextBand,
    pub stale: bool,
}

/// Format the complete textual indicator required by the UI policy.
///
/// The dashboard owns placement and colors; keeping the labels here ensures
/// every future surface uses the same estimate, warning, critical, and stale
/// semantics.
pub fn format_context_indicator(snapshot: &SessionContextSnapshot) -> ContextIndicator {
    let estimate = if snapshot.provenance == ContextProvenance::Estimated {
        "~"
    } else {
        ""
    };
    let mut text = format!("Ctx {estimate}{}%", snapshot.percentage.get());
    match snapshot.band {
        ContextBand::Normal => {}
        ContextBand::Warning => text.push_str(" WARNING"),
        ContextBand::Critical => text.push_str(" CRITICAL"),
    }
    let stale = snapshot.freshness == ContextFreshness::Stale;
    if stale {
        text.push_str(" STALE");
    }

    ContextIndicator {
        text,
        band: snapshot.band,
        stale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_tracking::{
        ContextPercentage, ContextResetMetadata, SessionContextSnapshot,
    };
    use chrono::{TimeZone, Utc};
    use std::num::NonZeroU64;

    fn snapshot(
        percentage: u8,
        band: ContextBand,
        provenance: ContextProvenance,
        freshness: ContextFreshness,
    ) -> SessionContextSnapshot {
        let now = Utc.timestamp_opt(1_000, 0).single().unwrap();
        SessionContextSnapshot {
            used_tokens: u64::from(percentage) * 1_000,
            context_limit: NonZeroU64::new(100_000).unwrap(),
            percentage: ContextPercentage::clamped(i64::from(percentage)),
            band,
            provenance,
            freshness,
            sampled_at: now,
            checked_at: now,
            reset: ContextResetMetadata::default(),
        }
    }

    #[test]
    fn formats_normal_direct_and_estimated_values() {
        assert_eq!(
            format_context_indicator(&snapshot(
                64,
                ContextBand::Normal,
                ContextProvenance::Direct,
                ContextFreshness::Fresh,
            ))
            .text,
            "Ctx 64%"
        );
        assert_eq!(
            format_context_indicator(&snapshot(
                64,
                ContextBand::Normal,
                ContextProvenance::Estimated,
                ContextFreshness::Fresh,
            ))
            .text,
            "Ctx ~64%"
        );
    }

    #[test]
    fn formats_warning_and_critical_labels() {
        assert_eq!(
            format_context_indicator(&snapshot(
                70,
                ContextBand::Warning,
                ContextProvenance::Direct,
                ContextFreshness::Fresh,
            ))
            .text,
            "Ctx 70% WARNING"
        );
        assert_eq!(
            format_context_indicator(&snapshot(
                85,
                ContextBand::Critical,
                ContextProvenance::Estimated,
                ContextFreshness::Fresh,
            ))
            .text,
            "Ctx ~85% CRITICAL"
        );
    }

    #[test]
    fn stale_direct_telemetry_is_never_presented_as_fresh() {
        let indicator = format_context_indicator(&snapshot(
            91,
            ContextBand::Critical,
            ContextProvenance::Direct,
            ContextFreshness::Stale,
        ));

        assert_eq!(indicator.text, "Ctx 91% CRITICAL STALE");
        assert!(indicator.stale);
    }
}
