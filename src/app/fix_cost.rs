//! Shared rendering of a per-issue **cost to fix** value and its "combined"
//! badge.
//!
//! Two surfaces show what one agent run cost to fix a single review issue: the
//! PR Triage reply disclosure (`R`) and, once a finding has been posted and
//! fixed, the AI Review pane (`W`). When several PR Triage comments are fixed
//! together in one combined batch (`B`), that single run's cost is shown in
//! full on **every** resolved comment in the batch, marked `combined` so it is
//! clear the figure is shared rather than counted once per issue.
//!
//! Both surfaces route their wording through here so the label and the badge
//! stay identical.

/// Combined-batch context for a fix cost: the issue was resolved as part of a
/// `B` batch whose single agent run's cost is shared across `sibling_count`
/// comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CombinedBatch {
    /// How many comments were fixed in the batch that this cost is shared
    /// across (always `>= 1`). Rendered as `(N)` after the badge when `> 1`.
    pub sibling_count: usize,
}

/// The label every surface uses for a per-issue fix cost. The `(est.)` keeps
/// the same "this is an estimate" hedge the raw token-cost figure has always
/// carried.
pub const FIX_COST_LABEL: &str = "Fix cost (est.)";

/// `combined` / `combined (3)` — the shared-cost marker on its own, for a
/// surface that renders the cost value itself.
pub fn combined_badge(batch: CombinedBatch) -> String {
    if batch.sibling_count > 1 {
        format!("combined ({})", batch.sibling_count)
    } else {
        "combined".to_string()
    }
}

/// The full relabeled line: `Fix cost (est.): $0.04 · combined (3)`, or
/// `Fix cost (est.): unavailable` when the harness reported no priceable
/// usage. The `· combined` marker is appended only when `batch` is `Some`.
pub fn fix_cost_line(cost: Option<&str>, batch: Option<CombinedBatch>) -> String {
    let value = cost.unwrap_or("unavailable");
    let mut line = format!("{FIX_COST_LABEL}: {value}");
    if let Some(batch) = batch {
        line.push_str(" · ");
        line.push_str(&combined_badge(batch));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_cost_line_has_no_badge() {
        assert_eq!(fix_cost_line(Some("$0.04"), None), "Fix cost (est.): $0.04");
    }

    #[test]
    fn missing_cost_reads_unavailable() {
        assert_eq!(fix_cost_line(None, None), "Fix cost (est.): unavailable");
        assert_eq!(
            fix_cost_line(None, Some(CombinedBatch { sibling_count: 2 })),
            "Fix cost (est.): unavailable · combined (2)"
        );
    }

    #[test]
    fn combined_badge_shows_count_only_past_one() {
        assert_eq!(
            combined_badge(CombinedBatch { sibling_count: 1 }),
            "combined"
        );
        assert_eq!(
            combined_badge(CombinedBatch { sibling_count: 3 }),
            "combined (3)"
        );
        assert_eq!(
            fix_cost_line(Some("$0.10"), Some(CombinedBatch { sibling_count: 1 })),
            "Fix cost (est.): $0.10 · combined"
        );
        assert_eq!(
            fix_cost_line(Some("$0.10"), Some(CombinedBatch { sibling_count: 3 })),
            "Fix cost (est.): $0.10 · combined (3)"
        );
    }
}
