//! Finishing a review round (`f`): either send the refinement brief to the
//! developer, or approve. The brief goes where Final Review's feedback goes:
//! the reviewer picks a destination from the same rows
//! (`review_destination_rows`) and the paste runs through the same
//! `dispatch_review_feedback`. Nothing is written until the reviewer confirms,
//! and the verdict is stored before the paste, so a failed paste never loses
//! the brief.

use crate::app::pr_review::FixTarget;
use crate::app::{App, ReviewDestinationPickState, ReviewDestinationRow};
use crate::design_review::agenda::{ItemKind, ItemState, Staleness};
use crate::design_review::brief::{
    BriefInput, RequestKind, brief_relative_path, build_brief, deliver, item_tag,
};

use super::actions::ROLE_REVIEWER;
use super::room::{
    DesignReviewRoom, FinishStage, FinishState, VERDICT_APPROVED, VERDICT_CHANGES_REQUESTED,
};

/// How many items still wait on the reviewer, off-agenda items aside.
fn awaiting_count(room: &DesignReviewRoom) -> usize {
    room.items
        .iter()
        .filter(|i| i.stale != Staleness::Retired && ItemState::parse(&i.state).awaits_reviewer())
        .count()
}

/// The destination as stored on the round, and the dispatch parameters.
fn route(
    row: &ReviewDestinationRow,
) -> Option<(
    FixTarget,
    Option<String>,
    Option<crate::project::AgentKind>,
    String,
)> {
    match row {
        ReviewDestinationRow::ExistingLive(_) => {
            Some((FixTarget::ExistingLive, None, None, "live".to_string()))
        }
        ReviewDestinationRow::Dedicated(agent) => Some((
            FixTarget::DedicatedReview,
            None,
            Some(agent.clone()),
            format!("dedicated:{}", agent.display_name()),
        )),
        ReviewDestinationRow::ExistingFeature { feature_id, .. } => Some((
            FixTarget::ExistingFeature,
            Some(feature_id.clone()),
            None,
            format!("feature:{feature_id}"),
        )),
        // The room offers no new-feature row: its setup is a diff-viewer overlay.
        ReviewDestinationRow::NewFeature => None,
    }
}

impl App {
    /// `f`: start finishing the round. Changes or questions open -> pick a
    /// destination for the brief; everything accepted -> confirm approval;
    /// otherwise say what is left.
    pub fn design_review_begin_finish(&mut self) {
        let prepared = match &self.mode {
            crate::app::AppMode::DesignReview(room) => Some(self.prepare_finish(room)),
            _ => None,
        };
        match prepared {
            Some(Ok(finish)) => {
                if let Some(room) = self.design_review_room_mut() {
                    room.finish = Some(finish);
                }
            }
            Some(Err(message)) => self.push_toast_warning(message),
            None => {}
        }
    }

    fn prepare_finish(&self, room: &DesignReviewRoom) -> Result<FinishState, String> {
        if room.read_only() {
            return Err("This review is approved and read-only".to_string());
        }
        let Some(db) = self.db.as_ref() else {
            return Err("Design Review needs the AMF database, which is unavailable".to_string());
        };
        let mut inputs = Vec::new();
        for item in &room.items {
            if item.stale == Staleness::Retired {
                continue;
            }
            let request = match ItemState::parse(&item.state) {
                ItemState::ChangeRequested => RequestKind::Change,
                ItemState::QuestionOpen => RequestKind::Question,
                _ => continue,
            };
            let text = db
                .design_review_turns(&item.id)
                .map_err(|err| format!("Cannot read the thread of {}: {err}", item.title))?
                .into_iter()
                .rev()
                .find(|turn| turn.role == ROLE_REVIEWER)
                .map(|turn| turn.body)
                .unwrap_or_default();
            let decision = room.decision_for(item).map(|d| (d.seq, d.title.as_str()));
            inputs.push(BriefInput {
                item_id: item.id.clone(),
                kind: request,
                existing_label: item.label.clone(),
                tag: item_tag(ItemKind::parse(&item.kind), decision, &item.title, request),
                text,
            });
        }

        if inputs.is_empty() {
            let waiting = awaiting_count(room);
            if waiting > 0 {
                return Err(format!(
                    "{waiting} item(s) still wait on you: accept (a) or request a change (c) first"
                ));
            }
            let open_decisions = room.decisions.iter().filter(|d| d.status == "open").count();
            return Ok(FinishState {
                stage: FinishStage::ConfirmApprove,
                brief: None,
                open_decisions,
            });
        }

        // Labels held by the round's items outside this brief stay theirs.
        let reserved: Vec<String> = room
            .items
            .iter()
            .filter(|item| !inputs.iter().any(|input| input.item_id == item.id))
            .filter_map(|item| item.label.clone())
            .collect();
        let brief = build_brief(room.round, &inputs, &reserved);
        let source = self.feature_indices_for_workdir(&room.workdir);
        let rows = self.review_destination_rows(source, false);
        Ok(FinishState {
            stage: FinishStage::Destination(ReviewDestinationPickState { rows, selected: 0 }),
            brief: Some(brief),
            open_decisions: 0,
        })
    }

    /// Move the destination highlight, wrapping.
    pub fn design_review_finish_move(&mut self, delta: isize) {
        if let Some(room) = self.design_review_room_mut()
            && let Some(FinishState {
                stage: FinishStage::Destination(pick),
                ..
            }) = room.finish.as_mut()
        {
            let n = pick.rows.len() as isize;
            if n > 0 {
                pick.selected = (pick.selected as isize + delta).rem_euclid(n) as usize;
            }
        }
    }

    /// Esc: from the dispatch confirmation back to the destinations,
    /// otherwise close the finish flow. Nothing has been written.
    pub fn design_review_finish_cancel(&mut self) {
        let Some(room) = self.design_review_room_mut() else {
            return;
        };
        let Some(finish) = room.finish.as_mut() else {
            return;
        };
        match &finish.stage {
            FinishStage::ConfirmSend(pick) => {
                finish.stage = FinishStage::Destination(pick.clone());
            }
            _ => room.finish = None,
        }
    }

    /// Enter: pick the destination, confirm the dispatch, or confirm approval.
    pub fn design_review_finish_confirm(&mut self) {
        let Some(stage) = self
            .design_review_room_mut()
            .and_then(|room| room.finish.as_ref().map(|f| f.stage.clone()))
        else {
            return;
        };
        match stage {
            FinishStage::Destination(pick) => {
                if let Some(room) = self.design_review_room_mut()
                    && let Some(finish) = room.finish.as_mut()
                    && pick.rows.get(pick.selected).is_some()
                {
                    finish.stage = FinishStage::ConfirmSend(pick);
                }
            }
            FinishStage::ConfirmSend(pick) => {
                if let Some(row) = pick.rows.get(pick.selected).cloned() {
                    self.design_review_send_brief(row);
                }
            }
            FinishStage::ConfirmApprove => self.design_review_approve(),
        }
    }

    /// Persist the verdict, the labels, the brief and its destination, then
    /// paste the brief the way Final Review pastes its feedback.
    fn design_review_send_brief(&mut self, row: ReviewDestinationRow) {
        let Some((fix_target, target_feature, harness, destination)) = route(&row) else {
            self.push_toast_warning("That destination is not available from the review room");
            return;
        };
        let prepared = match &self.mode {
            crate::app::AppMode::DesignReview(room) => room
                .finish
                .as_ref()
                .and_then(|f| f.brief.clone())
                .map(|brief| {
                    (
                        brief,
                        room.round_id.clone(),
                        room.workdir.clone(),
                        room.from_view.clone(),
                    )
                }),
            _ => None,
        };
        let Some((brief, round_id, workdir, from_view)) = prepared else {
            return;
        };

        let file_path = workdir.join(brief_relative_path(brief.round));
        let delivery = deliver(&brief, &file_path.to_string_lossy());
        if let Some((relative, contents)) = &delivery.file {
            let path = workdir.join(relative);
            let written = path
                .parent()
                .map(crate::design_review::outbox::ensure_dir)
                .transpose()
                .and_then(|_| {
                    std::fs::write(&path, contents)
                        .map_err(|err| anyhow::anyhow!("cannot write {}: {err}", path.display()))
                });
            if let Err(err) = written {
                self.log_error("design-review", format!("cannot write the brief: {err:#}"));
                self.push_toast_error("Could not write the brief file; nothing was sent");
                return;
            }
        }

        let Some(db) = self.db.as_ref() else {
            self.log_error(
                "design-review",
                "cannot save the verdict: no database".to_string(),
            );
            self.push_toast_error("Could not save the verdict to the database; nothing was sent");
            return;
        };
        let now = crate::db::learning::now_timestamp();
        let stored = db.design_review_transaction(|db| {
            for (item_id, label) in &brief.labels {
                db.set_design_review_item_label(item_id, label)?;
            }
            db.finish_design_review_round(
                &round_id,
                VERDICT_CHANGES_REQUESTED,
                Some(&brief.text),
                Some(&destination),
                &now,
            )
        });
        if let Err(err) = stored {
            self.log_error("design-review", format!("cannot save the verdict: {err:#}"));
            self.push_toast_error("Could not save the verdict to the database; nothing was sent");
            return;
        }

        let (changes, questions) = brief.labels.iter().fold((0, 0), |(c, q), (_, label)| {
            if label.starts_with('C') {
                (c + 1, q)
            } else {
                (c, q + 1)
            }
        });
        if let Some(room) = self.design_review_room_mut() {
            for (item_id, label) in &brief.labels {
                if let Some(item) = room.items.iter_mut().find(|i| &i.id == item_id) {
                    item.label = Some(label.clone());
                }
            }
            room.verdict = Some(VERDICT_CHANGES_REQUESTED.to_string());
            room.finish = None;
        }
        let summary = format!(
            "Design review round {}: brief sent ({changes} change(s), {questions} question(s))",
            brief.round
        );
        self.dispatch_review_feedback(
            from_view,
            summary,
            fix_target,
            target_feature,
            harness,
            Some(delivery.inline),
        );
    }

    /// Approve: the round's verdict becomes `approved` and every open decision
    /// of the feature `accepted`. Superseded decisions are untouched.
    pub(crate) fn design_review_approve(&mut self) {
        let Some((round_id, feature_id)) = (match &self.mode {
            crate::app::AppMode::DesignReview(room) => {
                Some((room.round_id.clone(), room.feature_id.clone()))
            }
            _ => None,
        }) else {
            return;
        };
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let now = crate::db::learning::now_timestamp();
        let outcome = db.design_review_transaction(|db| {
            let accepted = db.accept_open_design_decisions(&feature_id)?;
            db.finish_design_review_round(&round_id, VERDICT_APPROVED, None, None, &now)?;
            Ok(accepted)
        });
        match outcome {
            Ok(accepted) => {
                let decisions = db.design_decisions_for_feature(&feature_id).ok();
                if let Some(room) = self.design_review_room_mut() {
                    room.verdict = Some(VERDICT_APPROVED.to_string());
                    room.finish = None;
                    if let Some(decisions) = decisions {
                        room.decisions = decisions;
                    }
                }
                self.push_toast_info(format!("Design approved: {accepted} decision(s) accepted"));
            }
            Err(err) => {
                self.log_error("design-review", format!("cannot approve: {err:#}"));
                self.push_toast_error("Could not save the approval; nothing changed");
            }
        }
    }
}

/// The room's rows that count toward a verdict, for the header.
pub fn verdict_label(verdict: Option<&str>) -> Option<&'static str> {
    match verdict {
        Some(VERDICT_APPROVED) => Some("Approved"),
        Some(VERDICT_CHANGES_REQUESTED) => Some("Changes requested"),
        _ => None,
    }
}
