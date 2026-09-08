use super::navigation::{AnchorTarget, check_anchor_drift, load_file_lines};
use super::workers::thread_rows;
use crate::app::{
    App, AppMode, BrowseScope, LearningAnchor, LearningAnchorDrift, LearningFocus, LearningLevel,
    LearningListEntry, LearningQa, LearningViewState, Selection,
};
use crate::project::AgentKind;
use anyhow::Result;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

impl LearningViewState {
    /// A freshly opened overlay: repo-tree scope, cursor on the first entry,
    /// nothing loaded yet.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_id: String,
        pi: usize,
        fi: usize,
        project_name: String,
        feature_name: String,
        workdir: PathBuf,
        is_git: bool,
        harness: AgentKind,
        level: LearningLevel,
        session_id: String,
    ) -> Self {
        Self {
            project_id,
            pi,
            fi,
            project_name,
            feature_name,
            workdir,
            is_git,
            scope: BrowseScope::RepoTree,
            entries: Vec::new(),
            selected_entry: 0,
            list_scroll: 0,
            start_here_collapsed: false,
            expanded_dirs: BTreeSet::new(),
            expanded_seeded: false,
            repo_files: Vec::new(),
            start_here: Vec::new(),
            diff_files: Vec::new(),
            content: Vec::new(),
            content_path: None,
            content_scroll: 0,
            content_error: None,
            cursor_line: 0,
            selection_anchor: None,
            anchor: LearningAnchor::File,
            focus: LearningFocus::FileList,
            question: None,
            qa: Vec::new(),
            anchor_drift: HashMap::new(),
            selected_qa: 0,
            qa_scroll: 0,
            answer_open: false,
            answer_scroll: 0,
            answer_rendered_width: 0,
            answer_rendered_lines: Vec::new(),
            harness,
            harness_picker: None,
            starter_picker: None,
            action_editor: None,
            level,
            session_id,
            help_open: false,
            help_scroll: 0,
            error: None,
            notice: None,
            notice_qa_id: None,
        }
    }

    /// Number of selectable lines in the content pane: file lines in repo-tree
    /// scope, addressable diff lines in branch-changes scope.
    pub fn selectable_line_count(&self) -> usize {
        match self.scope {
            BrowseScope::RepoTree => self.content.len(),
            BrowseScope::BranchChanges => self
                .selected_diff_file()
                .map(|f| f.addressable_lines().len())
                .unwrap_or(0),
        }
    }

    /// Whether the current anchor's text is a diff excerpt rather than plain
    /// source. True only for a hunk or line selection inside branch-changes
    /// scope — a whole-file anchor is the file itself in either scope.
    pub fn selection_is_diff(&self) -> bool {
        self.scope == BrowseScope::BranchChanges
            && matches!(
                self.anchor,
                LearningAnchor::Hunk { .. } | LearningAnchor::Lines { .. }
            )
    }

    /// What became of a stored row's anchor, if it no longer points where it
    /// was stored. `None` is the ordinary case.
    pub fn drift_for(&self, qa_id: &str) -> Option<LearningAnchorDrift> {
        self.anchor_drift.get(qa_id).copied()
    }

    /// The inclusive cursor span, as indices into the content pane.
    pub fn selected_span(&self) -> (usize, usize) {
        match self.selection_anchor {
            Some(anchor) => (anchor.min(self.cursor_line), anchor.max(self.cursor_line)),
            None => (self.cursor_line, self.cursor_line),
        }
    }
}

impl App {
    /// Open Learning Mode on the feature at `(pi, fi)`.
    ///
    /// Loads (or creates) the project's learning session and its Q&A history,
    /// then lists the project's files. Works without a DB — history is simply
    /// empty and nothing is persisted.
    pub fn open_learning_mode(&mut self, pi: usize, fi: usize) -> Result<()> {
        let Some((project_id, project_name, feature_id, feature_name, workdir, is_git, preferred)) =
            self.store
                .projects
                .get(pi)
                .and_then(|p| p.features.get(fi).map(|f| (p, f)))
                .map(|(project, feature)| {
                    (
                        project.id.clone(),
                        project.name.clone(),
                        feature.id.clone(),
                        feature.name.clone(),
                        feature.workdir.clone(),
                        project.is_git,
                        project.preferred_agent.clone(),
                    )
                })
        else {
            return Ok(());
        };

        // Pre-selected so the harness picker is optional, matching the
        // final-review harness pick.
        let default_harness = self
            .store
            .available_harnesses
            .first()
            .cloned()
            .unwrap_or(preferred);

        let session = self.load_or_create_learning_session(
            &project_id,
            &feature_id,
            &project_name,
            &default_harness,
        );
        let (session_id, harness, level) = match &session {
            Some(s) => (s.id.clone(), s.harness.clone(), s.level),
            None => (String::new(), default_harness, LearningLevel::Newcomer),
        };
        let qa = self.load_learning_qa(&session_id);
        let workdir_label = workdir.display().to_string();
        let session_persisted = !session_id.is_empty();

        let mut state = LearningViewState::new(
            project_id,
            pi,
            fi,
            project_name,
            feature_name,
            workdir,
            is_git,
            harness,
            level,
            session_id,
        );
        state.qa = qa;
        self.mode = AppMode::Learning(Box::new(state));
        self.learning_reload_entries();
        // Open on the first row that means something: the "tour this project"
        // question when the orientation group is showing, else the first file.
        // Landing on a group header would make the first thing a newcomer sees
        // an empty content pane.
        if let AppMode::Learning(state) = &mut self.mode {
            state.selected_entry = state
                .entries
                .iter()
                .position(|e| !matches!(e, LearningListEntry::StartHereHeader))
                .unwrap_or(0);
        }
        self.learning_load_selected_content();
        self.learning_check_anchor_drift();
        self.learning_show_onboarding_if_new();
        let (entries, history) = match &self.mode {
            AppMode::Learning(state) => (state.entries.len(), state.qa.len()),
            _ => (0, 0),
        };
        self.log_info(
            "learning",
            format!(
                "opened on {} ({entries} entries, {history} past question(s), {})",
                workdir_label,
                if session_persisted {
                    "history is being saved"
                } else {
                    "history is in memory only"
                }
            ),
        );
        Ok(())
    }
}

impl App {
    /// Open Learning Mode on whatever the dashboard has selected.
    ///
    /// A project row opens on the project's first feature: the files have to be
    /// read from some working directory, and the first feature is the one that
    /// reuses the repo itself. A project with no features has nothing to read,
    /// which is worth saying out loud rather than swallowing the keypress.
    pub fn open_learning_mode_for_selection(&mut self) -> Result<()> {
        let target = match &self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => Some((*pi, *fi)),
            Selection::Project(pi) => self
                .store
                .projects
                .get(*pi)
                .filter(|project| !project.features.is_empty())
                .map(|_| (*pi, 0)),
        };
        let Some((pi, fi)) = target else {
            self.log_warn(
                "learning",
                "asked to open on a project with no features — nothing to read".to_string(),
            );
            self.message =
                Some("Add a feature first — Learning Mode reads that feature's files".to_string());
            return Ok(());
        };
        self.open_learning_mode(pi, fi)
    }
}

impl App {
    /// Close the overlay and return to the dashboard with the feature it was
    /// opened from selected.
    pub fn close_learning_mode(&mut self) {
        if let AppMode::Learning(state) = &self.mode {
            self.selection = Selection::Feature(state.pi, state.fi);
        }
        self.mode = AppMode::Normal;
    }
}

impl App {
    /// The project's learning session, created on first open. `None` with no
    /// DB (tests), in which case the overlay runs entirely in memory.
    pub(super) fn load_or_create_learning_session(
        &mut self,
        project_id: &str,
        feature_id: &str,
        title: &str,
        harness: &AgentKind,
    ) -> Option<crate::db::learning::LearningSession> {
        let db = self.db.as_ref()?;
        match db.load_or_create_learning_session(
            project_id,
            feature_id,
            title,
            harness,
            LearningLevel::Newcomer,
        ) {
            Ok(session) => Some(session),
            Err(e) => {
                self.log_error(
                    "learning",
                    format!(
                        "failed to open the learning session for {title}: {e} \
                         (questions will still work, but nothing will be saved)"
                    ),
                );
                None
            }
        }
    }
}

impl App {
    pub(super) fn load_learning_qa(&mut self, session_id: &str) -> Vec<LearningQa> {
        if session_id.is_empty() {
            return Vec::new();
        }
        let Some(db) = self.db.as_ref() else {
            return Vec::new();
        };
        match db.learning_qa(session_id) {
            Ok(rows) => thread_rows(self.reconcile_interrupted_qa(rows)),
            Err(e) => {
                self.log_error(
                    "learning",
                    format!("failed to load past questions: {e} (starting with an empty history)"),
                );
                Vec::new()
            }
        }
    }
}

impl App {
    /// Fail the rows a previous process left mid-run.
    ///
    /// A queued or running row is only meaningful while the thread that would
    /// deliver its answer is alive. After a quit or a crash there is no such
    /// thread, but the row is still stored as in-flight — so it would show
    /// "thinking…" and count towards the in-flight total forever. Rows this
    /// process is genuinely still waiting on (the overlay was closed and
    /// reopened mid-run) are left alone.
    pub(super) fn reconcile_interrupted_qa(
        &mut self,
        mut rows: Vec<LearningQa>,
    ) -> Vec<LearningQa> {
        let stranded: Vec<LearningQa> = rows
            .iter_mut()
            .filter(|row| {
                row.status.is_in_flight() && !self.learning_runs_in_flight.contains(&row.id)
            })
            .map(|row| {
                row.status = crate::app::LearningQaStatus::Failed;
                row.error = Some(
                    "AMF stopped while this question was still being answered, \
                     so the answer never arrived. Ask it again."
                        .to_string(),
                );
                row.updated_at = crate::db::learning::now_timestamp();
                row.clone()
            })
            .collect();
        if !stranded.is_empty() {
            self.log_info(
                "learning",
                format!(
                    "reset {} unfinished question(s) left behind by an earlier session",
                    stranded.len()
                ),
            );
        }
        for row in &stranded {
            // Already logged; a reset that couldn't be written through still
            // leaves a usable overlay.
            let _ = self.persist_learning_qa(row);
        }
        rows
    }
}

impl App {
    /// Check every stored anchor against the working directory as it is now,
    /// and say what has moved.
    ///
    /// This is the other half of reconciling a loaded history with the current
    /// world — [`reconcile_interrupted_qa`](Self::reconcile_interrupted_qa)
    /// does it for runs, this does it for the code they were about. It runs
    /// here rather than inside the history load because it needs the workdir,
    /// which is only assembled once the overlay's state exists.
    ///
    /// Eager rather than on-selection, deliberately: the point of the marker is
    /// to be there *before* the user reads a row and believes its line numbers.
    /// The cost is one read per distinct file in the history, deduped below.
    pub(super) fn learning_check_anchor_drift(&mut self) {
        let (workdir, rows) = match &self.mode {
            AppMode::Learning(state) => (state.workdir.clone(), state.qa.clone()),
            _ => return,
        };
        if rows.is_empty() {
            return;
        }
        let mut targets: HashMap<String, AnchorTarget> = HashMap::new();
        let mut drift: HashMap<String, LearningAnchorDrift> = HashMap::new();
        for qa in &rows {
            let Some(path) = qa.file_path.clone() else {
                continue;
            };
            if !targets.contains_key(&path) {
                let full = workdir.join(&path);
                // `Path::exists()` answers "no" both to a deleted file and to
                // one this process simply can't stat — an unreadable parent
                // directory, say. Only the first of those is `Gone`; reporting
                // the second as a lost anchor would be the mode stating as fact
                // something it was never able to look at.
                let target = match std::fs::metadata(&full) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => AnchorTarget::Gone,
                    Err(e) => {
                        self.log_warn(
                            "learning",
                            format!(
                                "couldn't reach {path} to re-check its anchor: {e} \
                                 (past questions about it keep their stored lines)"
                            ),
                        );
                        AnchorTarget::Unreadable
                    }
                    Ok(_) => match load_file_lines(&full, &path) {
                        Ok(lines) => AnchorTarget::Lines(lines),
                        Err(e) => {
                            self.log_warn(
                                "learning",
                                format!(
                                    "couldn't re-check the anchor for {path}: {e} \
                                     (past questions about it keep their stored lines)"
                                ),
                            );
                            AnchorTarget::Unreadable
                        }
                    },
                };
                targets.insert(path.clone(), target);
            }
            if let Some(verdict) = check_anchor_drift(qa, &targets[&path]) {
                drift.insert(qa.id.clone(), verdict);
            }
        }
        let moved = drift.values().filter(|d| !d.is_lost()).count();
        let lost = drift.values().filter(|d| d.is_lost()).count();
        if let AppMode::Learning(state) = &mut self.mode {
            state.anchor_drift = drift;
        }
        if moved == 0 && lost == 0 {
            return;
        }
        self.log_info(
            "learning",
            format!("{moved} past question(s) re-anchored, {lost} lost their anchor"),
        );
        let mut parts = Vec::new();
        if moved > 0 {
            parts.push(format!(
                "{moved} moved with the code (the answer still fits)"
            ));
        }
        if lost > 0 {
            parts.push(format!(
                "{lost} no longer {} at code that is there",
                if lost == 1 { "points" } else { "point" }
            ));
        }
        let summary = format!(
            "The project changed since some of these were asked: {}. They are marked in Questions.",
            parts.join(", ")
        );
        // Not `learning_notice`: that clears `error`, and a file the overlay
        // couldn't open on the way in is about the screen in front of the user
        // right now, which outranks a note about history. The row markers carry
        // this either way — the banner only says where to look.
        if let AppMode::Learning(state) = &mut self.mode {
            state.notice = Some(summary);
            state.notice_qa_id = None;
        }
    }
}

impl App {
    pub fn learning_open_help(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.help_open = true;
            state.help_scroll = 0;
        }
    }
}

impl App {
    pub fn learning_close_help(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.help_open = false;
            state.help_scroll = 0;
        }
    }
}

impl App {
    pub fn learning_help_scroll(&mut self, delta: isize) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.help_scroll = (state.help_scroll as isize + delta).max(0) as usize;
        }
    }
}

impl App {
    /// On the project's first visit, open the help overlay unprompted and
    /// remember that it's been shown. A newcomer's discovery path is this
    /// overlay, not the source.
    pub(super) fn learning_show_onboarding_if_new(&mut self) {
        let (session_id, seen) = match &self.mode {
            AppMode::Learning(state) => (state.session_id.clone(), state.help_open),
            _ => return,
        };
        let _ = seen;
        if session_id.is_empty() {
            return;
        }
        // A failed lookup counts as "seen": showing the intro on every open is
        // a worse failure than never showing it, but it should not be silent.
        let looked_up = self
            .db
            .as_ref()
            .map(|db| db.learning_session_onboarding_seen(&session_id));
        let already_seen = match looked_up {
            Some(Ok(seen)) => seen,
            Some(Err(e)) => {
                self.log_warn(
                    "learning",
                    format!("couldn't tell whether the Learning Mode intro has been shown: {e}"),
                );
                true
            }
            None => true,
        };
        if already_seen {
            return;
        }
        self.learning_open_help();
        if let Some(db) = self.db.as_ref()
            && let Err(e) = db.set_learning_onboarding_seen(&session_id)
        {
            self.log_warn(
                "learning",
                format!("couldn't record that the Learning Mode intro was shown: {e}"),
            );
        }
    }
}

impl App {
    /// Flip between newcomer and familiar answers. Applies to later questions
    /// only — an answer already on screen is never rewritten under the user.
    pub fn learning_toggle_level(&mut self) {
        let Some((session_id, harness, level)) = (match &mut self.mode {
            AppMode::Learning(state) => {
                state.level = state.level.toggled();
                Some((state.session_id.clone(), state.harness.clone(), state.level))
            }
            _ => None,
        }) else {
            return;
        };
        self.persist_learning_settings(&session_id, &harness, level);
    }
}

impl App {
    /// Open the harness picker, pre-selected on the harness in use.
    pub fn learning_open_harness_picker(&mut self) {
        let harnesses = if self.store.available_harnesses.is_empty() {
            let AppMode::Learning(state) = &self.mode else {
                return;
            };
            vec![state.harness.clone()]
        } else {
            self.store.available_harnesses.clone()
        };
        if let AppMode::Learning(state) = &mut self.mode {
            let selected = harnesses
                .iter()
                .position(|h| *h == state.harness)
                .unwrap_or(0);
            state.harness_picker = Some(crate::app::LearningHarnessPicker {
                harnesses,
                selected,
            });
        }
    }
}

impl App {
    pub fn learning_harness_picker_move(&mut self, delta: isize) {
        if let AppMode::Learning(state) = &mut self.mode
            && let Some(picker) = &mut state.harness_picker
        {
            let len = picker.harnesses.len();
            if len == 0 {
                return;
            }
            let next = (picker.selected as isize + delta).rem_euclid(len as isize) as usize;
            picker.selected = next;
        }
    }
}

impl App {
    /// Accept the highlighted harness. Applies to later questions; anything
    /// already in flight finishes on the harness that started it.
    pub fn learning_harness_picker_confirm(&mut self) {
        let Some((session_id, harness, level)) = (match &mut self.mode {
            AppMode::Learning(state) => {
                let picked = state
                    .harness_picker
                    .as_ref()
                    .and_then(|p| p.harnesses.get(p.selected).cloned());
                state.harness_picker = None;
                picked.map(|harness| {
                    state.harness = harness.clone();
                    (state.session_id.clone(), harness, state.level)
                })
            }
            _ => None,
        }) else {
            return;
        };
        self.persist_learning_settings(&session_id, &harness, level);
    }
}

impl App {
    pub fn learning_close_harness_picker(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.harness_picker = None;
        }
    }
}

impl App {
    pub(super) fn persist_learning_settings(
        &mut self,
        session_id: &str,
        harness: &AgentKind,
        level: LearningLevel,
    ) {
        if session_id.is_empty() {
            return;
        }
        let Some(db) = self.db.as_ref() else { return };
        if let Err(e) = db.set_learning_session_settings(session_id, harness, level) {
            self.log_warn(
                "learning",
                format!("couldn't save your Learning Mode settings: {e}"),
            );
        }
    }
}
