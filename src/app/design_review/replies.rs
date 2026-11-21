//! Ingesting the developer's replies (`amf review reply`) into the review.
//! A reply names an item by its brief label (`C1`, `Q1`), not by round, so it
//! goes to the newest round that carries that label.

use anyhow::Result;

use crate::db::AmfDb;
use crate::db::design_review::{DesignDecision, DesignReviewTurn};
use crate::design_review::agenda::{ItemAction, ItemState, transition};
use crate::design_review::brief::{RequestKind, parse_label};
use crate::design_review::record::{ReplyKind, ReplyRecord, TITLE_CAP, WHY_CAP, truncate_chars};

use super::actions::ROLE_REVIEWER;

/// The role of the developer's own turns.
pub const ROLE_DEVELOPER: &str = "developer";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplyOutcome {
    Applied,
    /// Already ingested (its turn exists); nothing changed.
    Duplicate,
    /// No item in the feature carries the label.
    UnknownLabel,
    /// The reply kind does not fit the label (`--answer` on a change).
    Mismatch,
}

fn thread_text(kind: ReplyKind, text: &str) -> String {
    match kind {
        ReplyKind::Done => format!("Done: {text}"),
        ReplyKind::Pushback => format!("Pushback: {text}"),
        ReplyKind::Answer => format!("Answer: {text}"),
    }
}

/// Apply one reply in one transaction, idempotent by the reply's uid (the
/// developer turn is stored under it). A change item awaiting the developer
/// goes to `re-review` for both `done` and `pushback`. An answer is stored as
/// a `review`-source decision, so the same question is not asked twice, and
/// the question's item returns to `pending`.
pub(super) fn apply_reply(
    db: &AmfDb,
    project_id: &str,
    feature_id: &str,
    reply: &ReplyRecord,
) -> Result<ReplyOutcome> {
    if db.design_review_turn_exists(&reply.uid)? {
        return Ok(ReplyOutcome::Duplicate);
    }
    let Some((label, label_kind)) = parse_label(&reply.label) else {
        return Ok(ReplyOutcome::UnknownLabel);
    };
    let Some(item) = db.design_review_item_by_label(feature_id, &label)? else {
        return Ok(ReplyOutcome::UnknownLabel);
    };
    let fits = matches!(
        (label_kind, reply.kind),
        (RequestKind::Change, ReplyKind::Done | ReplyKind::Pushback)
            | (RequestKind::Question, ReplyKind::Answer)
    );
    if !fits {
        return Ok(ReplyOutcome::Mismatch);
    }

    let now = crate::db::learning::now_timestamp();
    let state = ItemState::parse(&item.state);
    db.design_review_transaction(|db| {
        db.append_design_review_turn(&DesignReviewTurn {
            id: reply.uid.clone(),
            item_id: item.id.clone(),
            round_id: Some(item.round_id.clone()),
            feature_id: item.feature_id.clone(),
            project_id: item.project_id.clone(),
            seq: 0,
            role: ROLE_DEVELOPER.to_string(),
            level: None,
            body: thread_text(reply.kind, &reply.text),
            status: "done".to_string(),
            usage_id: None,
            created_at: now.clone(),
        })?;
        db.set_design_review_item_reply(&item.id, reply.kind.as_str(), &reply.text, &now)?;
        match label_kind {
            RequestKind::Change => {
                if let Ok(next) = transition(state, ItemAction::DeveloperReplied) {
                    db.set_design_review_item_state(&item.id, next.as_str(), &now)?;
                }
            }
            RequestKind::Question => {
                let question = db
                    .design_review_turns(&item.id)?
                    .into_iter()
                    .rev()
                    .find(|turn| turn.role == ROLE_REVIEWER)
                    .map_or_else(|| item.title.clone(), |turn| turn.body);
                let back_to = ItemAction::Answered {
                    back_to: ItemState::Pending,
                };
                if let Ok(next) = transition(state, back_to) {
                    db.set_design_review_item_state(&item.id, next.as_str(), &now)?;
                }
                db.insert_design_decision_if_absent(&DesignDecision {
                    uid: format!("review:{}", reply.uid),
                    feature_id: feature_id.to_string(),
                    project_id: project_id.to_string(),
                    // Outside the agent's own numbering, which the CLI counts
                    // from the outbox.
                    seq: 0,
                    kind: "tradeoff".to_string(),
                    title: truncate_chars(&question, TITLE_CAP).0,
                    why: truncate_chars(&reply.text, WHY_CAP).0,
                    rejected: Vec::new(),
                    ask: None,
                    supersedes: None,
                    paths: item.paths.clone(),
                    head: None,
                    session_id: reply.session_id.clone(),
                    harness: "claude".to_string(),
                    source: "review".to_string(),
                    status: "open".to_string(),
                    created_at: reply.ts.clone(),
                })?;
            }
        }
        Ok(())
    })?;
    Ok(ReplyOutcome::Applied)
}
