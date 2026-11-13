//! Design Review's pure pieces (`docs/development/features/design-review.md`).
//!
//! - [`support`]: the one gate every surface asks ("is the mode available for
//!   this harness and project?") and the wording shown when it is not.
//! - [`record`]: a decision record, its field caps and the outbox line shape.
//! - [`outbox`]: the append-only JSONL file the `amf decision` CLI writes and
//!   AMF ingests by byte offset.
//! - [`capture`]: `amf decision add`, which runs outside the TUI, never opens
//!   `amf.db`, and derives the mechanical fields from git.
//!
//! App glue (turn-end ingest into `amf.db`) lives in `src/app/design_review/`.

pub mod capture;
pub mod outbox;
pub mod record;
pub mod support;

pub use support::{
    DESIGN_REVIEW_CLAUDE_ONLY, DESIGN_REVIEW_CONTAINER_UNAVAILABLE, design_review_supported,
    design_review_unavailable_reason, feature_design_review_active,
    feature_design_review_supported,
};
