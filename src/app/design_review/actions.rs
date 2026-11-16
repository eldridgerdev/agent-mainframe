//! Item actions in the review room: accept, note, request change, ask, and
//! opening linked files in the diff viewer. Every state change is written to
//! `amf.db` before the room's copy changes, so a failed write never shows a
//! state the next open would not.

use crate::db::design_review::{DesignReviewItem, DesignReviewTurn};
use crate::design_review::agenda::{
    ItemAction, ItemState, NO_RECORD_ANSWER, record_answer, transition,
};

use super::room::{DesignReviewRoom, InputKind, RoomInput, RoomTab};
use crate::app::{App, AppMode, DiffViewerState};

/// The role of the reviewer's own turns.
pub const ROLE_REVIEWER: &str = "reviewer";
/// The role of an L0 answer: the stored record, quoted.
pub const ROLE_RECORD: &str = "record";

/// An answer from the question ladder.
struct LadderAnswer {
    role: &'static str,
    level: &'static str,
    body: String,
}

/// The question ladder (plan section 4): a question is answered at the lowest
/// level that can answer it. Only L0, the lookup of the stored record, exists
/// here; the presenter levels (L1, L2) branch from this function once the
/// staff runner is merged. A decision's record is quoted as is; an item
/// with no record says so rather than inventing a motive.
fn answer_question(room: &DesignReviewRoom, item: &DesignReviewItem) -> LadderAnswer {
    let body = match room.decision_for(item) {
        Some(decision) => record_answer(decision),
        None => NO_RECORD_ANSWER.to_string(),
    };
    LadderAnswer {
        role: ROLE_RECORD,
        level: "L0",
        body,
    }
}

fn action_for(kind: InputKind) -> ItemAction {
    match kind {
        InputKind::Note => ItemAction::Note,
        InputKind::RequestChange => ItemAction::RequestChange,
        InputKind::Ask => ItemAction::Ask,
    }
}

impl App {
    /// Start collecting text for a note, a change request or a question,
    /// unless the item's state refuses it.
    pub fn design_review_begin_input(&mut self, kind: InputKind) {
        let refusal = {
            let Some(room) = self.design_review_room_mut() else {
                return;
            };
            if room.tab != RoomTab::Agenda {
                return;
            }
            match room.selected_item() {
                None => Some("Nothing selected".to_string()),
                Some(item) => transition(ItemState::parse(&item.state), action_for(kind))
                    .err()
                    .map(str::to_string),
            }
        };
        match refusal {
            Some(message) => self.push_toast_warning(message),
            None => {
                if let Some(room) = self.design_review_room_mut() {
                    room.input = Some(RoomInput {
                        kind,
                        text: String::new(),
                    });
                }
            }
        }
    }

    pub fn design_review_input_char(&mut self, c: char) {
        if let Some(input) = self.design_review_room_mut().and_then(|r| r.input.as_mut()) {
            input.text.push(c);
        }
    }

    pub fn design_review_input_backspace(&mut self) {
        if let Some(input) = self.design_review_room_mut().and_then(|r| r.input.as_mut()) {
            input.text.pop();
        }
    }

    pub fn design_review_input_clear(&mut self) {
        if let Some(input) = self.design_review_room_mut().and_then(|r| r.input.as_mut()) {
            input.text.clear();
        }
    }

    pub fn design_review_input_cancel(&mut self) {
        if let Some(room) = self.design_review_room_mut() {
            room.input = None;
        }
    }

    /// Submit the prompt. Empty text is refused and keeps the prompt open.
    pub fn design_review_input_submit(&mut self) {
        let Some(input) = self.design_review_room_mut().and_then(|r| r.input.clone()) else {
            return;
        };
        let text = input.text.trim().to_string();
        if text.is_empty() {
            self.push_toast_warning("Type something first, or Esc to cancel");
            return;
        }
        if self.design_review_apply(action_for(input.kind), Some(text))
            && let Some(room) = self.design_review_room_mut()
        {
            room.input = None;
        }
    }

    /// Accept the selected item as it is.
    pub fn design_review_accept(&mut self) {
        self.design_review_apply(ItemAction::Accept, None);
    }

    /// Apply `action` to the selected item and persist it. Returns whether
    /// it was applied; a refusal or a failed write is toasted.
    fn design_review_apply(&mut self, action: ItemAction, text: Option<String>) -> bool {
        let prepared = match &self.mode {
            AppMode::DesignReview(room) => room.selected_item().map(|item| {
                let next = transition(ItemState::parse(&item.state), action);
                let answer = (action == ItemAction::Ask).then(|| answer_question(room, item));
                (item.clone(), next, answer)
            }),
            _ => None,
        };
        let Some((item, next, answer)) = prepared else {
            return false;
        };
        let next = match next {
            Ok(next) => next,
            Err(message) => {
                self.push_toast_warning(message);
                return false;
            }
        };
        let Some(db) = self.db.as_ref() else {
            return false;
        };
        let now = crate::db::learning::now_timestamp();
        let turn = |role: &str, level: Option<&str>, body: String| DesignReviewTurn {
            id: uuid::Uuid::new_v4().to_string(),
            item_id: item.id.clone(),
            round_id: Some(item.round_id.clone()),
            feature_id: item.feature_id.clone(),
            project_id: item.project_id.clone(),
            seq: 0,
            role: role.to_string(),
            level: level.map(str::to_string),
            body,
            status: "done".to_string(),
            usage_id: None,
            created_at: now.clone(),
        };

        // One transaction, so a failed write leaves none of the action behind
        // and the toast's "nothing changed" is true.
        let outcome = db.design_review_transaction(|db| -> anyhow::Result<ItemState> {
            match (action, text, answer) {
                (ItemAction::Accept, _, _) => {}
                (ItemAction::Note | ItemAction::RequestChange, Some(text), _) => {
                    db.append_design_review_turn(&turn(ROLE_REVIEWER, None, text))?;
                }
                (ItemAction::Ask, Some(question), Some(answer)) => {
                    db.append_design_review_turn(&turn(ROLE_REVIEWER, None, question))?;
                    db.set_design_review_item_state(&item.id, next.as_str(), &now)?;
                    db.append_design_review_turn(&turn(
                        answer.role,
                        Some(answer.level),
                        answer.body,
                    ))?;
                    let back_to = ItemState::parse(&item.state);
                    let back = transition(next, ItemAction::Answered { back_to })
                        .map_err(|message| anyhow::anyhow!(message))?;
                    db.set_design_review_item_state(&item.id, back.as_str(), &now)?;
                    return Ok(back);
                }
                _ => anyhow::bail!("this action needs text"),
            }
            db.set_design_review_item_state(&item.id, next.as_str(), &now)?;
            Ok(next)
        });

        match outcome {
            Ok(state) => {
                let mut warning = None;
                if let AppMode::DesignReview(room) = &mut self.mode {
                    if let Some(item) = room.items.get_mut(room.selected) {
                        item.state = state.as_str().to_string();
                    }
                    match self.db.as_ref().map(|db| db.design_review_turns(&item.id)) {
                        Some(Ok(turns)) => room.turns = turns,
                        Some(Err(err)) => warning = Some(format!("cannot read thread: {err}")),
                        None => {}
                    }
                    room.thread_scroll = 0;
                }
                if let Some(warning) = warning {
                    self.log_warn("design-review", warning);
                }
                true
            }
            Err(err) => {
                self.log_error(
                    "design-review",
                    format!("cannot save review action: {err:#}"),
                );
                self.push_toast_error("Could not save that to the database; nothing changed");
                false
            }
        }
    }

    /// Open the selected item's linked files in the diff viewer, on the first
    /// one. Esc in the viewer returns to the room.
    pub fn design_review_open_files(&mut self) {
        let focus = {
            let Some(room) = self.design_review_room_mut() else {
                return;
            };
            room.selected_item()
                .and_then(|item| item.paths.first().cloned())
        };
        let Some(focus) = focus else {
            self.push_toast_info("This item has no linked files");
            return;
        };
        let mode = std::mem::replace(&mut self.mode, AppMode::Normal);
        let AppMode::DesignReview(room) = mode else {
            self.mode = mode;
            return;
        };
        let mut viewer = DiffViewerState::new(room.from_view.clone(), room.workdir.clone());
        viewer.layout = self.preferred_diff_viewer_layout();
        viewer.focus_path = Some(focus);
        viewer.return_to = Some(Box::new(AppMode::DesignReview(room)));
        self.mode = AppMode::DiffViewerLoading(viewer);
    }
}
