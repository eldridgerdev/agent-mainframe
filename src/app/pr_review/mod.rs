//! PR comment-review model and normalization (feature-specific).
//!
//! The generic `gh` access lives in [`crate::github`]; this module turns those
//! raw GitHub payloads into a single triage-ready [`PrReview`] and owns the
//! token-saving transforms (bot-boilerplate stripping, one-line snippets,
//! thread-resolution merge). See `docs/backlog/pr-comment-review-plan.md`.

// Some helpers (token estimate for the confirm dialog, the loading-state probe)
// are consumed by later epics; keep them until those land.
#![allow(dead_code)]

mod actions;
mod domain;
mod fetch;
mod integration;
mod investigation;
mod memory;
mod reply;
pub(crate) mod state;

#[cfg(test)]
mod tests;
use actions::{new_fix_confirm, with_reply_draft_handoff};
pub(crate) use domain::AI_ATTRIBUTION_FOOTER;
pub(crate) use domain::AI_REVIEW_ATTRIBUTION_FOOTER;
pub(crate) use domain::AMF_ATTRIBUTION_FOOTER;
pub(crate) use domain::TRIAGE_SESSION_LABEL;
pub(crate) use domain::fix_session_index;
pub(crate) use domain::pr_triage_session_index;
pub(crate) use domain::pr_triage_session_index_named;
pub(crate) use domain::window_parsed_hunk;
use domain::{
    BATCH_COMBINED_COMMENT_WARN, BATCH_COMBINED_TOKEN_WARN, SNIPPET_LEN, append_reply_attribution,
    pr_triage_session_index_named_for_harness,
};
pub use domain::{
    CommentKind, FixTarget, FixTargetPickRow, MarkAction, PrComment, PrInvestigationStatus,
    PrInvestigationTurn, PrReview, PrSortMode, ReplyDraftProvenance, ReplyDraftRequest,
    ReplyGenerationMetadata, ReplyKind, ReplyTarget, TriageState, combined_fix_prompt,
    estimate_tokens, reply_effective_agent_drafted,
};
pub use fetch::{fetch_and_normalize, strip_bot_boilerplate};
pub use investigation::InvestigationOutcome;
pub(crate) use investigation::investigation_findings_for_prompt;
pub(crate) use memory::MEMORY_CATEGORIES;
pub use memory::{
    BootstrapDepth, BootstrapProgress, BootstrapStage, CompactProgress, CompactStage,
};

#[cfg(test)]
pub use domain::reply_posted_via_amf;
#[cfg(test)]
pub use fetch::normalize;
#[cfg(test)]
pub use memory::{BootstrapOutcome, CompactOutcome};

pub(crate) mod runtime;
