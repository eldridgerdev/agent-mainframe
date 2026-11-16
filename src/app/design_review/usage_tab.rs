//! The room's Usage tab: loading the ledger, grouping, drilling into a
//! group's calls and copying the table. The aggregation is pure and lives in
//! `crate::design_review::usage`.

use crate::design_review::usage;

use crate::app::{App, AppMode};

impl App {
    /// Re-read the feature's ledger rows and the header total.
    pub fn design_review_reload_usage(&mut self) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let AppMode::DesignReview(room) = &mut self.mode else {
            return;
        };
        match db.design_review_usage(&room.feature_id) {
            Ok(rows) => {
                room.cost_total = usage::header_total(&rows);
                room.usage.rows = rows;
                let groups = room.usage.table().groups.len();
                room.usage.selected = room.usage.selected.min(groups.saturating_sub(1));
                if room.usage.drill.is_some_and(|g| g >= groups) {
                    room.usage.drill = None;
                }
            }
            Err(err) => {
                let message = format!("cannot read the usage ledger: {err}");
                self.log_warn("design-review", message);
                self.push_toast_error("Could not read the usage ledger");
            }
        }
    }

    /// `g`: the next grouping. The selection returns to the first row.
    pub fn design_review_cycle_grouping(&mut self) {
        if let Some(room) = self.design_review_room_mut() {
            room.usage.grouping = room.usage.grouping.next();
            room.usage.selected = 0;
            room.usage.drill = None;
            room.usage.call_selected = 0;
        }
    }

    /// Move the selection in the group table, or in a group's calls.
    pub fn design_review_usage_move(&mut self, delta: isize) {
        let Some(room) = self.design_review_room_mut() else {
            return;
        };
        let table = room.usage.table();
        match room.usage.drill {
            Some(group) => {
                let calls = table.groups.get(group).map_or(0, |g| g.rows.len());
                room.usage.call_selected = room
                    .usage
                    .call_selected
                    .saturating_add_signed(delta)
                    .min(calls.saturating_sub(1));
            }
            None => {
                room.usage.selected = room
                    .usage
                    .selected
                    .saturating_add_signed(delta)
                    .min(table.groups.len().saturating_sub(1));
            }
        }
    }

    /// Enter: open the selected group's calls.
    pub fn design_review_usage_drill(&mut self) {
        let Some(room) = self.design_review_room_mut() else {
            return;
        };
        if room.usage.drill.is_none() && room.usage.selected < room.usage.table().groups.len() {
            room.usage.drill = Some(room.usage.selected);
            room.usage.call_selected = 0;
        }
    }

    /// Leave a group's calls. Returns whether there was one to leave.
    pub fn design_review_usage_back(&mut self) -> bool {
        match self.design_review_room_mut() {
            Some(room) if room.usage.drill.is_some() => {
                room.usage.drill = None;
                true
            }
            _ => false,
        }
    }

    /// `y`: copy the table as Markdown.
    pub fn design_review_usage_copy(&mut self) {
        let Some(markdown) = self
            .design_review_room_mut()
            .map(|room| usage::to_markdown(&room.usage.table()))
        else {
            return;
        };
        match crate::app::util::copy_to_clipboard(&markdown) {
            Ok(()) => self.push_toast_success("Usage table copied as Markdown"),
            Err(err) => self.push_toast_error(format!("Could not copy the usage table: {err}")),
        }
    }
}
