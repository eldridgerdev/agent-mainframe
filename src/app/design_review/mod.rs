//! Design Review App glue (`docs/development/features/design-review.md`).
//!
//! Phase 1 owns only ingest: moving the agent's outbox lines into `amf.db`.
//! The pure pieces (gate, record caps, outbox format, CLI) live in
//! `crate::design_review`.

use crate::db::design_review::DesignDecision;
use crate::design_review::outbox;
use crate::design_review::record::OutboxLine;

use super::App;

impl App {
    /// Ingest new Decision Log lines from the feature's outbox into `amf.db`,
    /// resuming at the stored byte offset. Idempotent by uid, so re-reading a
    /// line (a truncated outbox, a lost offset write) never duplicates it.
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
        let Some(db) = self.db.as_ref() else {
            return 0;
        };

        let offset = match db.design_review_outbox_offset(&feature_id) {
            Ok(offset) => offset,
            Err(err) => {
                self.log_warn(
                    "design-review",
                    format!("cannot read outbox offset for {feature_id}: {err}"),
                );
                return 0;
            }
        };
        let (lines, next_offset, malformed) = match outbox::read_from(&path, offset) {
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
        let mut failures = Vec::new();
        for line in &lines {
            match line {
                OutboxLine::Decision(record) => {
                    let row = DesignDecision::from_record(&project_id, &feature_id, record);
                    match db.insert_design_decision_if_absent(&row) {
                        Ok(true) => inserted += 1,
                        Ok(false) => {}
                        Err(err) => failures.push(format!("{}: {err}", record.uid)),
                    }
                }
            }
        }
        // Keep the old offset when a row failed to insert, so the next turn
        // retries it; the uid makes the retry safe for the rows that landed.
        let offset_err = if failures.is_empty() && next_offset != offset {
            db.set_design_review_outbox_offset(&project_id, &feature_id, next_offset)
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
        if inserted > 0 {
            self.log_info(
                "design-review",
                format!("ingested {inserted} decision(s) for feature {feature_id}"),
            );
        }
        inserted
    }
}
