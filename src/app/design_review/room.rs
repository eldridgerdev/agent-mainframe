//! The review room's state and lifecycle: opening (gate, ingest, round,
//! agenda), loading the selected item, and closing. Item actions live in
//! `actions.rs`, the Usage tab in `usage_tab.rs`. Keys are in
//! `crate::handlers::design_review`, drawing in `crate::ui::dialogs`.

use std::path::PathBuf;

use anyhow::Result;
use ratatui::text::Line;

use crate::db::AmfDb;
use crate::db::design_review::{
    DesignDecision, DesignReviewItem, DesignReviewRound, DesignReviewTurn,
};
use crate::design_packet::context::DecisionSummary;
use crate::design_packet::detectors;
use crate::design_review::agenda::{
    self, AgendaInputs, ItemKind, ItemState, NOT_PREPARED, StoredItem, derive_agenda, merge_agenda,
};
use crate::design_review::usage::{self, Grouping, UsageRow};
use crate::design_review::{DESIGN_REVIEW_CLAUDE_ONLY, design_review_unavailable_reason};

use super::packet::{PacketStatus, SectionState};
use super::presenter::Escalation;
use super::snapshot::{RoomSnapshot, load_room_snapshot};
use crate::app::{App, AppMode, ViewState};

/// The header's fixed label.
pub const HEADER_LABEL: &str = "Design Review";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomTab {
    Agenda,
    Usage,
}

/// What the text prompt at the bottom is collecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Note,
    RequestChange,
    Ask,
}

impl InputKind {
    pub fn prompt(self) -> &'static str {
        match self {
            InputKind::Note => "Note (Enter accepts with this note)",
            InputKind::RequestChange => "Requested change (Enter sends it)",
            InputKind::Ask => "Question (Enter asks)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomInput {
    pub kind: InputKind,
    pub text: String,
}

/// The Usage tab's view state. The rows are loaded from the ledger when the
/// tab opens and on `r`.
pub struct UsageTabState {
    pub rows: Vec<UsageRow>,
    pub grouping: Grouping,
    /// Selected row of the group table (the total row is not selectable).
    pub selected: usize,
    /// The group whose calls are open (an index into the table's groups).
    pub drill: Option<usize>,
    pub call_selected: usize,
    /// First visible row of whichever list is shown; the renderer moves it
    /// to keep the selection on screen.
    pub scroll: usize,
}

impl UsageTabState {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            grouping: Grouping::default(),
            selected: 0,
            drill: None,
            call_selected: 0,
            scroll: 0,
        }
    }

    pub fn table(&self) -> usage::UsageTable {
        usage::group_usage(&self.rows, self.grouping)
    }
}

impl Default for UsageTabState {
    fn default() -> Self {
        Self::new()
    }
}

/// A room waiting for its git snapshot, which is read off the UI thread so a
/// large diff or repository does not freeze the TUI while the room opens.
pub(crate) struct DesignReviewOpening {
    feature_id: String,
    rx: std::sync::mpsc::Receiver<RoomSnapshot>,
}

pub struct DesignReviewRoom {
    /// The session view the room was opened from; closing returns to it.
    pub from_view: ViewState,
    pub feature_id: String,
    pub project_name: String,
    pub feature_name: String,
    pub workdir: PathBuf,
    pub round: i64,
    pub items: Vec<DesignReviewItem>,
    pub selected: usize,
    pub list_scroll: usize,
    pub decisions: Vec<DesignDecision>,
    /// The selected item's thread.
    pub turns: Vec<DesignReviewTurn>,
    /// Markdown shown in the content pane for the selected item.
    pub content: String,
    pub content_scroll: usize,
    pub content_width: u16,
    pub content_lines: Vec<Line<'static>>,
    pub diagrams: crate::mermaid::DiagramDocument,
    /// Lines the thread is scrolled up from its end.
    pub thread_scroll: usize,
    pub tab: RoomTab,
    pub input: Option<RoomInput>,
    pub help_open: bool,
    pub usage: UsageTabState,
    /// The header's one-line cost total.
    pub cost_total: String,
    /// The packet's section states, copied from the runtime on each poll so
    /// the renderer reads no runtime or database.
    pub packet: PacketStatus,
    /// A priced presenter escalation waiting for its confirm key.
    pub confirm: Option<Escalation>,
    /// The git snapshot the room opened with: re-deriving the agenda and the
    /// presenter's linked hunks read it.
    pub(crate) snapshot: RoomSnapshot,
}

impl DesignReviewRoom {
    pub fn selected_item(&self) -> Option<&DesignReviewItem> {
        self.items.get(self.selected)
    }

    pub fn decision_for(&self, item: &DesignReviewItem) -> Option<&DesignDecision> {
        let uid = item.ref_key.as_deref()?;
        self.decisions.iter().find(|d| d.uid == uid)
    }

    /// The packet state of the selected item's section, if it is one.
    pub fn section_state(&self, item: &DesignReviewItem) -> Option<&SectionState> {
        let section = ItemKind::parse(&item.kind)?.section_key()?;
        self.packet.state(section)
    }

    /// Reset everything derived from the selected item's content.
    fn clear_content_cache(&mut self) {
        self.content_scroll = 0;
        self.content_lines.clear();
        self.content_width = 0;
        self.diagrams.reset();
        self.diagrams.pan = 0;
        self.thread_scroll = 0;
    }
}

fn stored(item: &DesignReviewItem) -> StoredItem {
    StoredItem {
        id: item.id.clone(),
        kind: item.kind.clone(),
        ref_key: item.ref_key.clone(),
        label: item.label.clone(),
        title: item.title.clone(),
        paths: item.paths.clone(),
        state: ItemState::parse(&item.state),
    }
}

fn summaries(decisions: &[DesignDecision]) -> Vec<DecisionSummary> {
    decisions
        .iter()
        .map(|d| DecisionSummary {
            seq: d.seq.max(0) as u32,
            kind: d.kind.clone(),
            title: d.title.clone(),
            paths: d.paths.clone(),
        })
        .collect()
}

/// The agenda's suffix for a packet section that has no settled artifact
/// yet: preparing, failed or skipped. `None` once it is ready.
pub fn section_badge(state: &SectionState) -> Option<&'static str> {
    match state {
        SectionState::Running => Some(" preparing…"),
        SectionState::Failed(_) => Some(" failed"),
        SectionState::Skipped(_) => Some(" skipped"),
        SectionState::Cached | SectionState::Done => None,
    }
}

/// What a section with no stored artifact shows, given its packet state.
fn unprepared_text(state: Option<&SectionState>) -> String {
    match state {
        Some(SectionState::Running) => format!("{NOT_PREPARED}. The packet is being prepared…"),
        Some(SectionState::Failed(reason)) => {
            format!("{NOT_PREPARED}: preparation failed.\n\n{reason}")
        }
        Some(SectionState::Skipped(reason)) => format!("{NOT_PREPARED}: skipped.\n\n{reason}"),
        _ => NOT_PREPARED.to_string(),
    }
}

/// Show agenda item titles, not ids, in the Usage tab's item column.
pub(crate) fn label_usage_items(rows: &mut [UsageRow], items: &[DesignReviewItem]) {
    for row in rows {
        if let Some(item) = row
            .item
            .as_deref()
            .and_then(|id| items.iter().find(|i| i.id == id))
        {
            row.item = Some(item.title.clone());
        }
    }
}

/// Derive the agenda for a round and persist it, keeping every stored item's
/// id, state and thread. Returns the round's items in agenda order.
pub(crate) fn derive_and_store_agenda(
    db: &AmfDb,
    round: &DesignReviewRound,
    snapshot: &RoomSnapshot,
) -> Result<Vec<DesignReviewItem>> {
    let decisions = db.design_decisions_for_feature(&round.feature_id)?;
    let hits = snapshot
        .files
        .as_ref()
        .map(|files| detectors::detect(files, &snapshot.base_dirs))
        .unwrap_or_default();
    let uncovered = detectors::uncovered(&hits, &summaries(&decisions));
    let has_unexplained_artifact = db
        .latest_design_review_artifact(&round.feature_id, "unexplained")?
        .is_some();
    let drafts = derive_agenda(&AgendaInputs {
        decisions: &decisions,
        uncovered_hits: &uncovered,
        has_unexplained_artifact,
    });

    let existing = db.design_review_items(&round.id)?;
    let stored_items: Vec<StoredItem> = existing.iter().map(stored).collect();
    let current = snapshot.changed_paths();
    let planned = merge_agenda(&stored_items, &drafts, current.as_deref());

    let now = crate::db::learning::now_timestamp();
    // One transaction: a failure partway must not leave a half-reordered
    // agenda for the next open to build on.
    db.design_review_transaction(|db| {
        for (position, plan) in planned.iter().enumerate() {
            let item = DesignReviewItem {
                id: plan
                    .existing_id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                round_id: round.id.clone(),
                feature_id: round.feature_id.clone(),
                project_id: round.project_id.clone(),
                position: position as i64,
                kind: plan.kind.clone(),
                ref_key: plan.ref_key.clone(),
                label: plan.label.clone(),
                title: plan.title.clone(),
                paths: plan.paths.clone(),
                state: plan.state.as_str().to_string(),
                stale: plan.stale,
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            if plan.existing_id.is_some() {
                db.update_design_review_item_layout(&item)?;
            } else {
                db.insert_design_review_item(&item)?;
            }
        }
        Ok(())
    })?;
    db.design_review_items(&round.id)
}

impl App {
    /// Prepare the packet sections for `round` of the feature at (`pi`,
    /// `fi`) in the background ([`App::prepare_design_review_packet`]). The
    /// room calls it on open; [`App::design_review_sync_packet`] follows the
    /// progress from the poll path.
    pub(crate) fn request_design_review_packet(&mut self, pi: usize, fi: usize, round: i64) {
        match u32::try_from(round) {
            Ok(round) => self.prepare_design_review_packet(pi, fi, round),
            Err(_) => self.log_warn(
                "design-review",
                format!("round {round} is out of range; packet not prepared"),
            ),
        }
    }

    /// Leader `y` from a session view: read the viewed feature's git snapshot
    /// in the background; [`App::poll_design_review_opening`] opens the room
    /// when it arrives. A refusal is toasted at once.
    pub fn open_design_review_from_view(&mut self) {
        let AppMode::Viewing(view) = &self.mode else {
            return;
        };
        let view = view.clone();
        let target = self.store.projects.iter().enumerate().find_map(|(pi, p)| {
            (p.name == view.project_name).then(|| {
                p.features
                    .iter()
                    .position(|f| f.name == view.feature_name)
                    .map(|fi| (pi, fi))
            })?
        });
        let Some((pi, fi)) = target else {
            self.push_toast_warning("No active feature to review");
            return;
        };
        if self.design_review_opening.is_some() {
            self.push_toast_info("Design Review is already opening");
            return;
        }
        if !self.design_review_may_open(pi, fi) {
            return;
        }
        let feature = &self.store.projects[pi].features[fi];
        let feature_id = feature.id.clone();
        let workdir = feature.workdir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(load_room_snapshot(&workdir));
        });
        self.design_review_opening = Some(DesignReviewOpening { feature_id, rx });
        self.push_toast_info("Reading the diff for Design Review…");
    }

    /// Open the room once its background snapshot has arrived, if the
    /// reviewer is still viewing that feature. Returns whether anything
    /// changed on screen.
    pub fn poll_design_review_opening(&mut self) -> bool {
        let Some(opening) = &self.design_review_opening else {
            return false;
        };
        let snapshot = match opening.rx.try_recv() {
            Ok(snapshot) => snapshot,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.design_review_opening = None;
                self.log_error("design-review", "snapshot reader stopped".to_string());
                self.push_toast_error("Design Review could not read the diff; see the debug log");
                return true;
            }
        };
        let Some(DesignReviewOpening { feature_id, .. }) = self.design_review_opening.take() else {
            return false;
        };
        let target = self.store.projects.iter().enumerate().find_map(|(pi, p)| {
            p.features
                .iter()
                .position(|f| f.id == feature_id)
                .map(|fi| (pi, fi, p.name.clone(), p.features[fi].name.clone()))
        });
        let view = match (&self.mode, target) {
            (AppMode::Viewing(view), Some((pi, fi, project, feature)))
                if view.project_name == project && view.feature_name == feature =>
            {
                Some((pi, fi, view.clone()))
            }
            _ => None,
        };
        let Some((pi, fi, view)) = view else {
            self.log_info(
                "design-review",
                "left the feature before the room opened; not opening it".to_string(),
            );
            return false;
        };
        self.open_design_review(pi, fi, view, Some(snapshot))
    }

    /// The open gate: toasts and returns false when Design Review is
    /// unavailable for the feature's harness or project, off for the
    /// feature, or without a database.
    fn design_review_may_open(&mut self, pi: usize, fi: usize) -> bool {
        let Some((project, feature)) = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi).map(|f| (p, f)))
        else {
            return false;
        };
        if let Some(reason) =
            design_review_unavailable_reason(&feature.agent, project.containerized)
        {
            self.push_toast_warning(format!("Design Review is {reason}"));
            return false;
        }
        if !feature.design_review {
            self.push_toast_warning("Design Review is off for this feature");
            return false;
        }
        if self.db.is_none() {
            self.push_toast_warning("Design Review needs the AMF database, which is unavailable");
            return false;
        }
        true
    }

    /// Open the room for a feature. Refuses with a toast when Design Review
    /// is unavailable for its harness or project, or off for the feature.
    /// `snapshot` is read from git, on this thread, when `None`. Returns
    /// whether it opened.
    pub(crate) fn open_design_review(
        &mut self,
        pi: usize,
        fi: usize,
        from_view: ViewState,
        snapshot: Option<RoomSnapshot>,
    ) -> bool {
        if !self.design_review_may_open(pi, fi) {
            return false;
        }
        let project = &self.store.projects[pi];
        let feature = &project.features[fi];
        let project_id = project.id.clone();
        let project_name = project.name.clone();
        let feature_id = feature.id.clone();
        let feature_name = feature.name.clone();
        let workdir = feature.workdir.clone();

        self.ingest_design_review_outbox(pi, fi);
        let snapshot = snapshot.unwrap_or_else(|| load_room_snapshot(&workdir));

        let round = match self.ensure_design_review_round(&project_id, &feature_id, &snapshot) {
            Ok(round) => round,
            Err(err) => {
                self.log_error("design-review", format!("cannot open round: {err:#}"));
                self.push_toast_error("Design Review could not open its round; see the debug log");
                return false;
            }
        };
        self.request_design_review_packet(pi, fi, round.round);

        let Some(db) = self.db.as_ref() else {
            return false;
        };
        let loaded = derive_and_store_agenda(db, &round, &snapshot).and_then(|items| {
            Ok((
                items,
                db.design_decisions_for_feature(&feature_id)?,
                db.design_review_usage(&feature_id)?,
            ))
        });
        let (items, decisions, mut usage_rows) = match loaded {
            Ok(loaded) => loaded,
            Err(err) => {
                self.log_error("design-review", format!("cannot build agenda: {err:#}"));
                self.push_toast_error(
                    "Design Review could not build its agenda; see the debug log",
                );
                return false;
            }
        };

        let cost_total = usage::header_total(&usage_rows);
        label_usage_items(&mut usage_rows, &items);
        let packet = self.design_review_packet_status(&feature_id);
        let mut room = DesignReviewRoom {
            from_view,
            feature_id,
            project_name,
            feature_name,
            workdir,
            round: round.round,
            items,
            selected: 0,
            list_scroll: 0,
            decisions,
            turns: Vec::new(),
            content: String::new(),
            content_scroll: 0,
            content_width: 0,
            content_lines: Vec::new(),
            diagrams: Default::default(),
            thread_scroll: 0,
            tab: RoomTab::Agenda,
            input: None,
            help_open: false,
            usage: UsageTabState {
                rows: usage_rows,
                ..UsageTabState::new()
            },
            cost_total,
            packet,
            confirm: None,
            snapshot,
        };
        // Resume at the first item still waiting on the reviewer.
        room.selected = room
            .items
            .iter()
            .position(|i| ItemState::parse(&i.state).awaits_reviewer())
            .unwrap_or(0);
        self.mode = AppMode::DesignReview(Box::new(room));
        self.design_review_load_selected();
        true
    }

    /// The feature's latest round, created as round 1 on first open.
    fn ensure_design_review_round(
        &self,
        project_id: &str,
        feature_id: &str,
        snapshot: &RoomSnapshot,
    ) -> Result<DesignReviewRound> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no database"))?;
        if let Some(round) = db.design_review_latest_round(feature_id)? {
            return Ok(round);
        }
        let round = DesignReviewRound {
            id: uuid::Uuid::new_v4().to_string(),
            feature_id: feature_id.to_string(),
            project_id: project_id.to_string(),
            round: 1,
            base_commit: snapshot.base_commit.clone(),
            head_commit: snapshot.head_commit.clone(),
            diff_fingerprint: snapshot.fingerprint.clone(),
            verdict: None,
            created_at: crate::db::learning::now_timestamp(),
        };
        db.insert_design_review_round(&round)?;
        Ok(round)
    }

    /// Close the room and return to the session view it came from. The
    /// packet stops being tracked (runs in flight still reach the ledger);
    /// presenter answers in flight are still stored.
    pub fn close_design_review(&mut self) {
        let mode = std::mem::replace(&mut self.mode, AppMode::Normal);
        match mode {
            AppMode::DesignReview(room) => {
                self.cancel_design_review_packet(&room.feature_id);
                self.mode = AppMode::Viewing(room.from_view);
            }
            other => self.mode = other,
        }
    }

    /// Copy the packet's section states into the open room. When a section
    /// changes state, the selected item reloads if it is that section, and
    /// an `unexplained` section turning ready re-derives the agenda so its
    /// item appears. Called from the poll path; the renderer reads only the
    /// copy.
    pub(crate) fn design_review_sync_packet(&mut self) {
        let AppMode::DesignReview(room) = &self.mode else {
            return;
        };
        let status = self.design_review_packet_status(&room.feature_id);
        if status == room.packet {
            return;
        }
        let changed: Vec<&'static str> = status
            .sections
            .iter()
            .filter(|(key, state)| room.packet.state(key) != Some(state))
            .map(|(key, _)| *key)
            .collect();
        let ready = |key: &str| {
            matches!(
                status.state(key),
                Some(SectionState::Done | SectionState::Cached)
            )
        };
        let unexplained_ready = changed.contains(&"unexplained") && ready("unexplained");
        let selected_changed = room
            .selected_item()
            .and_then(|item| ItemKind::parse(&item.kind)?.section_key())
            .is_some_and(|key| changed.contains(&key));
        if let Some(room) = self.design_review_room_mut() {
            room.packet = status;
        }
        let mut reload = selected_changed;
        if unexplained_ready {
            reload |= self.design_review_rederive_agenda();
        }
        if reload {
            self.design_review_load_selected();
        }
    }

    /// Re-derive and store the open room's agenda, keeping the selection on
    /// the same item. Returns whether the selected item changed.
    fn design_review_rederive_agenda(&mut self) -> bool {
        let Some(db) = self.db.as_ref() else {
            return false;
        };
        let AppMode::DesignReview(room) = &mut self.mode else {
            return false;
        };
        let derived = db
            .design_review_latest_round(&room.feature_id)
            .and_then(|round| {
                let round = round.ok_or_else(|| anyhow::anyhow!("no round"))?;
                derive_and_store_agenda(db, &round, &room.snapshot)
            });
        match derived {
            Ok(items) => {
                let selected_id = room.selected_item().map(|i| i.id.clone());
                room.items = items;
                let position = selected_id
                    .and_then(|id| room.items.iter().position(|i| i.id == id))
                    .unwrap_or(0);
                let moved = position != room.selected;
                room.selected = position.min(room.items.len().saturating_sub(1));
                moved
            }
            Err(err) => {
                let message = format!("cannot refresh the agenda: {err:#}");
                self.log_warn("design-review", message);
                false
            }
        }
    }

    pub(crate) fn design_review_room_mut(&mut self) -> Option<&mut DesignReviewRoom> {
        match &mut self.mode {
            AppMode::DesignReview(room) => Some(room),
            _ => None,
        }
    }

    /// Move the agenda selection by `delta`, clamped, and load the new item.
    pub fn design_review_move(&mut self, delta: isize) {
        let Some(room) = self.design_review_room_mut() else {
            return;
        };
        if room.items.is_empty() {
            return;
        }
        let next = room
            .selected
            .saturating_add_signed(delta)
            .min(room.items.len() - 1);
        if next != room.selected {
            room.selected = next;
            self.design_review_load_selected();
        }
    }

    /// Load the selected item's content and thread. Read again on every
    /// selection, so an artifact written after the room opened shows up.
    pub(crate) fn design_review_load_selected(&mut self) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let AppMode::DesignReview(room) = &mut self.mode else {
            return;
        };
        room.clear_content_cache();
        let Some(item) = room.items.get(room.selected).cloned() else {
            room.content = String::new();
            room.turns.clear();
            return;
        };
        let kind = ItemKind::parse(&item.kind);
        let decision = room.decision_for(&item).cloned();
        let mut errors = Vec::new();
        room.content = match (kind, decision) {
            (Some(ItemKind::Decision), Some(d)) => agenda::decision_markdown(&d),
            (Some(ItemKind::Ask), Some(d)) => agenda::ask_markdown(&d),
            (Some(ItemKind::Decision | ItemKind::Ask), None) => {
                "This decision is no longer recorded.".to_string()
            }
            (Some(ItemKind::UnexplainedHit), _) => agenda::hit_markdown(&item.title, &item.paths),
            (Some(section_kind), _) => match section_kind.section_key() {
                Some(section) => {
                    match db.latest_design_review_artifact(&room.feature_id, section) {
                        Ok(Some(artifact)) if artifact.format == "markdown" => artifact.body,
                        Ok(Some(artifact)) => format!("```\n{}\n```", artifact.body),
                        Ok(None) => unprepared_text(room.packet.state(section)),
                        Err(err) => {
                            errors.push(format!("cannot read {section} artifact: {err}"));
                            NOT_PREPARED.to_string()
                        }
                    }
                }
                None => NOT_PREPARED.to_string(),
            },
            (None, _) => NOT_PREPARED.to_string(),
        };
        match db.design_review_turns(&item.id) {
            Ok(turns) => room.turns = turns,
            Err(err) => {
                room.turns.clear();
                errors.push(format!("cannot read thread: {err}"));
            }
        }
        for error in errors {
            self.log_warn("design-review", error);
        }
    }

    /// Scroll the content pane by `delta` lines.
    pub fn design_review_scroll_content(&mut self, delta: isize) {
        if let Some(room) = self.design_review_room_mut() {
            room.content_scroll = room.content_scroll.saturating_add_signed(delta);
        }
    }

    /// Scroll the thread by `delta` lines (positive is older).
    pub fn design_review_scroll_thread(&mut self, delta: isize) {
        if let Some(room) = self.design_review_room_mut() {
            room.thread_scroll = room.thread_scroll.saturating_add_signed(delta);
        }
    }

    /// Switch between the agenda and the Usage tab. Entering Usage reloads
    /// the ledger, so the figures are current.
    pub fn design_review_set_tab(&mut self, tab: RoomTab) {
        if tab == RoomTab::Usage {
            self.design_review_reload_usage();
        }
        if let Some(room) = self.design_review_room_mut() {
            room.tab = tab;
            room.input = None;
        }
    }

    pub fn design_review_toggle_help(&mut self) {
        if let Some(room) = self.design_review_room_mut() {
            room.help_open = !room.help_open;
        }
    }

    /// Pan wide diagrams, or flip them to source.
    pub fn design_review_pan(&mut self, key: crossterm::event::KeyEvent) -> bool {
        let Some(room) = self.design_review_room_mut() else {
            return false;
        };
        let moved = room.diagrams.handle_pan_key(key);
        if moved {
            room.content_lines.clear();
        }
        moved
    }

    pub fn design_review_toggle_diagram_source(&mut self) {
        if let Some(room) = self.design_review_room_mut() {
            room.diagrams.toggle_source();
            room.content_lines.clear();
        }
    }
}

/// The header's fixed title, with the harness limit.
pub fn header_label() -> String {
    format!("{HEADER_LABEL} · {DESIGN_REVIEW_CLAUDE_ONLY}")
}
