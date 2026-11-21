//! Design Review App glue (`docs/development/features/design-review.md`).
//!
//! Ingest moves the agent's outbox lines into `amf.db` (Phase 1); `packet`
//! prepares the review packet with staff runs, the artifact cache and the
//! ledger (Phase 2). The review room (`room`, `actions`, `usage_tab`) reads
//! the records back as an agenda. The pure pieces (gate, record caps, outbox
//! format, CLI, context builders) live in `crate::design_review` and
//! `crate::design_packet`.

// The review room's packet hook is the first non-test caller of this API.
#[allow(dead_code)]
pub(crate) mod packet;
mod packet_inputs;

use crate::db::design_review::DesignDecision;
use crate::design_review::outbox;
use crate::design_review::record::{OutboxLine, parse_decision_ref};

pub(crate) mod actions;
pub(crate) mod finish;
mod replies;
pub(crate) mod room;
mod snapshot;
pub(crate) mod usage_tab;

use super::App;

impl App {
    /// `requested` Design Review, kept only when the harness and the named
    /// project's container setting support it. The single place a creation
    /// path turns a request into a stored flag, so an unsupported feature is
    /// never persisted with the flag on.
    pub(crate) fn effective_design_review(
        &self,
        project_name: &str,
        agent: &crate::project::AgentKind,
        requested: bool,
    ) -> bool {
        requested
            && crate::design_review::design_review_supported(
                agent,
                self.store
                    .find_project(project_name)
                    .is_some_and(|project| project.containerized),
            )
    }

    /// Ingest new Decision Log and reply lines from the feature's outbox into `amf.db`,
    /// resuming at the stored cursor. Idempotent by uid, so re-reading a line
    /// (a replaced outbox, a lost cursor write) never duplicates it. The
    /// outbox is per checkout, so it is read only while this feature owns it
    /// ([`outbox::bind`]); another feature's decisions are never ingested as
    /// this one's.
    /// Malformed lines are logged and skipped. Returns how many decisions were
    /// new. Runs at agent turn end and is meant to run again when the review
    /// opens, which also covers harnesses without turn-end hooks.
    pub(crate) fn ingest_design_review_outbox(&mut self, pi: usize, fi: usize) -> usize {
        let Some(project) = self.store.projects.get(pi) else {
            return 0;
        };
        let Some(feature) = project.features.get(fi) else {
            return 0;
        };
        let project_id = project.id.clone();
        let feature_id = feature.id.clone();
        let path = outbox::outbox_path(&feature.workdir);
        if outbox::owner(&feature.workdir).as_deref() != Some(feature_id.as_str()) {
            return 0;
        }
        let Some(db) = self.db.as_ref() else {
            return 0;
        };

        let cursor = match db.design_review_outbox_cursor(&feature_id) {
            Ok(cursor) => cursor,
            Err(err) => {
                self.log_warn(
                    "design-review",
                    format!("cannot read outbox offset for {feature_id}: {err}"),
                );
                return 0;
            }
        };
        let outbox::OutboxRead {
            lines,
            cursor: next_cursor,
            malformed,
        } = match outbox::read_from(&path, &cursor) {
            Ok(read) => read,
            Err(err) => {
                self.log_warn(
                    "design-review",
                    format!("cannot read {}: {err}", path.display()),
                );
                return 0;
            }
        };

        let mut inserted = 0;
        let mut applied_replies = 0;
        let mut failures = Vec::new();
        let mut skipped = Vec::new();
        for line in &lines {
            match line {
                OutboxLine::Decision(record) => {
                    let row = DesignDecision::from_record(&project_id, &feature_id, record);
                    match db.insert_design_decision_if_absent(&row) {
                        Ok(true) => inserted += 1,
                        Ok(false) => {}
                        Err(err) => failures.push(format!("{}: {err}", record.uid)),
                    }
                    // A later record that names an earlier one replaces it.
                    // Re-applied on a re-read, which is a no-op.
                    if let Some(target) = record.supersedes.as_deref().and_then(parse_decision_ref)
                        && target < i64::from(record.seq)
                        && let Err(err) = db.mark_design_decision_superseded(&feature_id, target)
                    {
                        failures.push(format!("{} supersedes: {err}", record.uid));
                    }
                }
                OutboxLine::Reply(reply) => {
                    match replies::apply_reply(db, &project_id, &feature_id, reply) {
                        Ok(replies::ReplyOutcome::Applied) => applied_replies += 1,
                        Ok(replies::ReplyOutcome::Duplicate) => {}
                        Ok(replies::ReplyOutcome::UnknownLabel) => skipped.push(format!(
                            "reply {} names no item ({})",
                            reply.uid, reply.label
                        )),
                        Ok(replies::ReplyOutcome::Mismatch) => skipped
                            .push(format!("reply {} does not fit {}", reply.uid, reply.label)),
                        Err(err) => failures.push(format!("{}: {err}", reply.uid)),
                    }
                }
            }
        }
        // Keep the old offset when a row failed to insert, so the next turn
        // retries it; the uid makes the retry safe for the rows that landed.
        let offset_err = if failures.is_empty() && next_cursor != cursor {
            db.set_design_review_outbox_cursor(&project_id, &feature_id, &next_cursor)
                .err()
        } else {
            None
        };

        if malformed > 0 {
            self.log_warn(
                "design-review",
                format!(
                    "skipped {malformed} malformed line(s) in {}",
                    path.display()
                ),
            );
        }
        for failure in failures {
            self.log_warn(
                "design-review",
                format!("failed to store decision {failure}"),
            );
        }
        if let Some(err) = offset_err {
            self.log_warn(
                "design-review",
                format!("failed to store outbox offset for {feature_id}: {err}"),
            );
        }
        for reason in skipped {
            self.log_warn("design-review", format!("skipped {reason}"));
        }
        if applied_replies > 0 {
            self.log_info(
                "design-review",
                format!("ingested {applied_replies} reply(ies) for feature {feature_id}"),
            );
        }
        if inserted > 0 {
            self.log_info(
                "design-review",
                format!("ingested {inserted} decision(s) for feature {feature_id}"),
            );
        }
        inserted
    }
}

/// A snapshot whose diff is exactly `files`, for tests that need a known diff.
#[cfg(test)]
pub(crate) fn room_snapshot_for_test(files: Vec<crate::diff::DiffFile>) -> snapshot::RoomSnapshot {
    snapshot::RoomSnapshot {
        files: Some(files),
        ..Default::default()
    }
}
