//! Design Review's pure pieces (`docs/development/features/design-review.md`).
//!
//! - [`support`]: the one gate every surface asks ("is the mode available for
//!   this harness and project?") and the wording shown when it is not.
//! - [`record`]: a decision record, its field caps and the outbox line shape.
//! - [`outbox`]: the append-only JSONL file the `amf decision` CLI writes and
//!   AMF ingests by byte offset.
//! - [`agenda`] and [`usage`]: the review room's agenda derivation and item
//!   state machine, and the Usage tab's ledger aggregation.
//! - [`brief`]: the refinement brief a finished round sends the developer.
//! - [`capture`]: `amf decision add` and `amf review reply`, which run outside the TUI, never opens
//!   `amf.db`, and derives the mechanical fields from git.
//!
//! App glue (turn-end ingest into `amf.db`) lives in `src/app/design_review/`.

pub mod agenda;
pub mod brief;
pub mod capture;
pub mod outbox;
pub mod record;
pub mod support;
pub mod usage;

pub use support::{
    DESIGN_REVIEW_CLAUDE_ONLY, DESIGN_REVIEW_CONTAINER_UNAVAILABLE, PresetReview,
    design_review_supported, design_review_unavailable_reason, feature_design_review_active,
    feature_design_review_supported, resolve_preset_review,
};
