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
    text.push_str(" · ");
    text.push_str(&format_raw_token_count(snapshot.used_tokens));

    ContextIndicator {
        text,
        band: snapshot.band,
        stale,
    }
}

/// Render a raw token count with thousands separators and no unit suffix or
/// rounding, so the number stands on its own next to the severity label
/// regardless of band.
fn format_raw_token_count(tokens: u64) -> String {
    let digits = tokens.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().rev().enumerate() {
        if index != 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped.chars().rev().collect()
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
            "Ctx 64% · 64,000"
        );
        assert_eq!(
            format_context_indicator(&snapshot(
                64,
                ContextBand::Normal,
                ContextProvenance::Estimated,
                ContextFreshness::Fresh,
            ))
            .text,
            "Ctx ~64% · 64,000"
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
            "Ctx 70% WARNING · 70,000"
        );
        assert_eq!(
            format_context_indicator(&snapshot(
                85,
                ContextBand::Critical,
                ContextProvenance::Estimated,
                ContextFreshness::Fresh,
            ))
            .text,
            "Ctx ~85% CRITICAL · 85,000"
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

        assert_eq!(indicator.text, "Ctx 91% CRITICAL STALE · 91,000");
        assert!(indicator.stale);
    }

    #[test]
    fn raw_token_count_uses_thousands_separators_at_every_magnitude() {
        assert_eq!(format_raw_token_count(0), "0");
        assert_eq!(format_raw_token_count(999), "999");
        assert_eq!(format_raw_token_count(1_000), "1,000");
        assert_eq!(format_raw_token_count(184_320), "184,320");
        assert_eq!(format_raw_token_count(1_234_567), "1,234,567");
    }
}
