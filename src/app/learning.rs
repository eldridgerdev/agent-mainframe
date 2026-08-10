//! Learning Mode: a read-only browser over a project, with an agent answering
//! questions about whatever the cursor is on
//! (`docs/backlog/learning-mode-plan.md`).
//!
//! This module owns opening/closing the overlay, loading the file list for
//! each browse scope, loading file content, and moving the selection that a
//! question anchors to. Asking, answering, and everything downstream of an
//! answer land in later epics.
//!
//! **Nothing here writes to the repository.** File content is read, never
//! written; the only persistence is the learning session and its Q&A history
//! in `amf.db`. As with the TODOs overlay, the in-memory state is the source
//! of truth, so the mode works with no DB at all.
//!
//! The overlay has no key handler or renderer until the plan's Epic 4, so this
//! module is written before anything calls it. The allow comes off in Epic 6.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::app::{
    App, AppMode, BrowseScope, LearningAnchor, LearningFocus, LearningLevel, LearningListEntry,
    LearningListGroup, LearningQa, LearningQaIntent, LearningViewState, Selection,
};
use crate::diff::{DiffFile, DiffLineLocation};
use crate::project::AgentKind;

/// Files a newcomer should read first, checked for existence in the workdir
/// and pinned above the repo-tree file list. Ordered by how much orientation
/// each one usually gives, not alphabetically. Missing entries are simply
/// absent — see the plan's "the Start here candidate list is a heuristic".
pub const START_HERE_CANDIDATES: &[&str] = &[
    "README.md",
    "readme.md",
    "CLAUDE.md",
    "AGENTS.md",
    "CONTRIBUTING.md",
    "src/main.rs",
    "src/lib.rs",
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
];

/// Largest file the content pane will load. Past this, reading the file costs
/// more than it teaches, and the answer prompt couldn't carry it anyway.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// How much of a file is sniffed for a NUL byte before calling it binary.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Cap on the repo-tree file list. A monorepo browsed by an unfamiliar user is
/// the worst case for both listing cost and usefulness; the list is truncated
/// rather than allowed to grow without bound.
const MAX_REPO_ENTRIES: usize = 20_000;

/// Depth cap for the non-git fallback walk.
const MAX_WALK_DEPTH: usize = 12;

/// Directories the non-git fallback walk skips. Git projects get `.gitignore`
/// handling for free from `git ls-files`; a non-git project has no ignore
/// rules at all, so this short list stands in for the obvious noise.
const WALK_SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".cache",
];

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
            selected_qa: 0,
            qa_scroll: 0,
            answer_open: false,
            answer_scroll: 0,
            answer_rendered_width: 0,
            answer_rendered_lines: Vec::new(),
            harness,
            harness_picker: None,
            starter_picker: None,
            level,
            session_id,
            help_open: false,
            help_scroll: 0,
            error: None,
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
        self.learning_show_onboarding_if_new();
        Ok(())
    }

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
            self.message =
                Some("Add a feature first — Learning Mode reads that feature's files".to_string());
            return Ok(());
        };
        self.open_learning_mode(pi, fi)
    }

    /// Close the overlay and return to the dashboard with the feature it was
    /// opened from selected.
    pub fn close_learning_mode(&mut self) {
        if let AppMode::Learning(state) = &self.mode {
            self.selection = Selection::Feature(state.pi, state.fi);
        }
        self.mode = AppMode::Normal;
    }

    /// The project's learning session, created on first open. `None` with no
    /// DB (tests), in which case the overlay runs entirely in memory.
    fn load_or_create_learning_session(
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

    fn load_learning_qa(&mut self, session_id: &str) -> Vec<LearningQa> {
        if session_id.is_empty() {
            return Vec::new();
        }
        let Some(db) = self.db.as_ref() else {
            return Vec::new();
        };
        match db.learning_qa(session_id) {
            Ok(rows) => self.reconcile_interrupted_qa(rows),
            Err(e) => {
                self.log_error(
                    "learning",
                    format!("failed to load past questions: {e} (starting with an empty history)"),
                );
                Vec::new()
            }
        }
    }

    /// Fail the rows a previous process left mid-run.
    ///
    /// A queued or running row is only meaningful while the thread that would
    /// deliver its answer is alive. After a quit or a crash there is no such
    /// thread, but the row is still stored as in-flight — so it would show
    /// "thinking…" and count towards the in-flight total forever. Rows this
    /// process is genuinely still waiting on (the overlay was closed and
    /// reopened mid-run) are left alone.
    fn reconcile_interrupted_qa(&mut self, mut rows: Vec<LearningQa>) -> Vec<LearningQa> {
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
            self.persist_learning_qa(row);
        }
        rows
    }

    /// Switch between "all files in this project" and "files changed on this
    /// branch", reloading the list in place. Branch-changes scope needs git,
    /// so a non-git project stays where it is and says why.
    pub fn learning_toggle_scope(&mut self) {
        let is_git = match &self.mode {
            AppMode::Learning(state) => state.is_git,
            _ => return,
        };
        if let AppMode::Learning(state) = &mut self.mode {
            if state.scope == BrowseScope::RepoTree && !is_git {
                state.error = Some(
                    "This project isn't a git repository, so there are no branch changes to show."
                        .to_string(),
                );
                return;
            }
            state.scope = state.scope.toggled();
            state.selected_entry = 0;
            state.list_scroll = 0;
            state.error = None;
        }
        self.learning_reload_entries();
        self.learning_load_selected_content();
    }

    /// Rebuild the file list for the current scope.
    pub fn learning_reload_entries(&mut self) {
        let Some((scope, workdir, is_git, has_history, collapsed)) = (match &self.mode {
            AppMode::Learning(state) => Some((
                state.scope,
                state.workdir.clone(),
                state.is_git,
                !state.qa.is_empty(),
                state.start_here_collapsed,
            )),
            _ => None,
        }) else {
            return;
        };

        let mut load_error: Option<String> = None;
        let mut diff_files: Vec<DiffFile> = Vec::new();
        let entries = match scope {
            BrowseScope::BranchChanges => match crate::diff::load_snapshot(&workdir, None, false) {
                Ok(snapshot) => {
                    diff_files = snapshot.files;
                    build_changed_entries(&diff_files)
                }
                Err(e) => {
                    load_error = Some(format!(
                        "Couldn't list this branch's changes: {e}. \
                             Press the scope key to browse all files instead."
                    ));
                    Vec::new()
                }
            },
            BrowseScope::RepoTree => {
                let mut files = if is_git {
                    match crate::diff::list_repo_files(&workdir) {
                        Ok(files) => files,
                        Err(e) => {
                            load_error = Some(format!(
                                "Couldn't list this project's files: {e}. \
                                 Check that git is installed and this directory is a repository."
                            ));
                            Vec::new()
                        }
                    }
                } else {
                    walk_files_capped(&workdir, MAX_REPO_ENTRIES, MAX_WALK_DEPTH)
                };
                if let Some(total) = cap_repo_entries(&mut files, MAX_REPO_ENTRIES) {
                    load_error.get_or_insert(format!(
                        "This project has {total} files — showing the first {MAX_REPO_ENTRIES}. \
                         Switch to branch changes to see what's actually changed."
                    ));
                }
                let start_here = if has_history {
                    Vec::new()
                } else {
                    start_here_candidates(&workdir)
                };
                build_repo_tree_entries(&files, &start_here, collapsed)
            }
        };

        if let AppMode::Learning(state) = &mut self.mode {
            state.diff_files = diff_files;
            state.entries = entries;
            if state.selected_entry >= state.entries.len() {
                state.selected_entry = state.entries.len().saturating_sub(1);
            }
            state.error = load_error.clone();
        }
        if let Some(msg) = load_error {
            self.log_warn("learning", msg);
        }
    }

    /// Show or hide the pinned orientation group.
    pub fn learning_toggle_start_here(&mut self) {
        let selected_path = match &self.mode {
            AppMode::Learning(state) => state
                .selected_entry()
                .and_then(|e| e.path())
                .map(str::to_string),
            _ => return,
        };
        if let AppMode::Learning(state) = &mut self.mode {
            state.start_here_collapsed = !state.start_here_collapsed;
        }
        self.learning_reload_entries();
        // Keep the cursor on whatever file it was on, if that row survived.
        if let (AppMode::Learning(state), Some(path)) = (&mut self.mode, selected_path)
            && let Some(idx) = state
                .entries
                .iter()
                .position(|e| e.path() == Some(path.as_str()))
        {
            state.selected_entry = idx;
        }
    }

    pub fn learning_select_next_entry(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            if state.entries.is_empty() {
                return;
            }
            state.selected_entry = (state.selected_entry + 1) % state.entries.len();
        }
        self.learning_load_selected_content();
    }

    pub fn learning_select_prev_entry(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            if state.entries.is_empty() {
                return;
            }
            state.selected_entry = state
                .selected_entry
                .checked_sub(1)
                .unwrap_or(state.entries.len() - 1);
        }
        self.learning_load_selected_content();
    }

    /// Load the content for whatever the file-list cursor is on, resetting the
    /// content cursor and anchor to the top of the new file.
    pub fn learning_load_selected_content(&mut self) {
        let Some((entry, workdir, scope, diff_lines)) = (match &self.mode {
            AppMode::Learning(state) => state.selected_entry().map(|entry| {
                // Branch-changes scope renders the diff, but the *prompt* still
                // needs the file the diff sits in: without it a whole-file
                // anchor would carry only the lines the hunks happen to touch,
                // and a line anchor would have no surrounding context at all.
                // The snapshot already hydrated both sides, so this costs no
                // extra read — and it works for a deleted file, which the
                // working tree no longer has.
                let diff_lines = match (state.scope, entry) {
                    (
                        BrowseScope::BranchChanges,
                        LearningListEntry::File {
                            diff_index: Some(index),
                            ..
                        },
                    ) => state
                        .diff_files
                        .get(*index)
                        .map(diff_file_lines)
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                (
                    entry.clone(),
                    state.workdir.clone(),
                    state.scope,
                    diff_lines,
                )
            }),
            _ => None,
        }) else {
            return;
        };

        match entry {
            // The orientation rows aren't files: the tour question anchors to
            // the project, and the header only toggles the group.
            LearningListEntry::StartHereHeader => {}
            LearningListEntry::ProjectTour => {
                if let AppMode::Learning(state) = &mut self.mode {
                    state.content = Vec::new();
                    state.content_path = None;
                    state.content_error = None;
                    state.content_scroll = 0;
                    state.cursor_line = 0;
                    state.selection_anchor = None;
                    state.anchor = LearningAnchor::Project;
                }
            }
            LearningListEntry::File { path, .. } => {
                // The pane renders the diff in branch-changes scope, so the
                // file there comes from the snapshot rather than from disk.
                let loaded = match scope {
                    BrowseScope::BranchChanges => Ok(diff_lines),
                    BrowseScope::RepoTree => load_file_lines(&workdir.join(&path)),
                };
                if let AppMode::Learning(state) = &mut self.mode {
                    match loaded {
                        Ok(lines) => {
                            state.content = lines;
                            state.content_error = None;
                        }
                        Err(reason) => {
                            state.content = Vec::new();
                            state.content_error = Some(reason);
                        }
                    }
                    state.content_path = Some(path);
                    state.content_scroll = 0;
                    state.cursor_line = 0;
                    state.selection_anchor = None;
                    state.anchor = LearningAnchor::File;
                }
            }
        }
    }

    /// Move the content cursor, clearing nothing — an in-progress range
    /// extends as the cursor moves, which is Final Review's interaction.
    pub fn learning_cursor_move(&mut self, delta: isize) {
        if let AppMode::Learning(state) = &mut self.mode {
            let count = state.selectable_line_count();
            if count == 0 {
                return;
            }
            let next = (state.cursor_line as isize + delta).clamp(0, count as isize - 1) as usize;
            state.cursor_line = next;
            // Moving the cursor re-anchors to the line (or span) under it,
            // unless the user has explicitly taken the whole file or project.
            if !matches!(state.anchor, LearningAnchor::Project) {
                let anchor = anchor_for_cursor(state);
                state.anchor = anchor;
            }
        }
    }

    /// Start (or restart) a multi-line selection at the cursor.
    pub fn learning_start_range(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            if state.selectable_line_count() == 0 {
                return;
            }
            state.selection_anchor = Some(state.cursor_line);
            let anchor = anchor_for_cursor(state);
            state.anchor = anchor;
        }
    }

    /// Drop a multi-line selection back to the cursor line.
    pub fn learning_clear_range(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.selection_anchor = None;
            if state.selectable_line_count() > 0 {
                let anchor = anchor_for_cursor(state);
                state.anchor = anchor;
            }
        }
    }

    /// Anchor the next question to the whole current file.
    pub fn learning_select_whole_file(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode
            && state.content_path.is_some()
        {
            state.selection_anchor = None;
            state.anchor = LearningAnchor::File;
        }
    }

    /// Anchor the next question to the project as a whole.
    pub fn learning_select_project(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.selection_anchor = None;
            state.anchor = LearningAnchor::Project;
        }
    }

    /// Anchor to the hunk containing the cursor. Only meaningful in
    /// branch-changes scope — repo-tree browsing has no diff, so this reports
    /// that rather than silently doing nothing.
    pub fn learning_select_hunk(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            if !state.hunk_selection_available() {
                state.error = Some(
                    "Hunks only exist for changed files. Switch to branch changes to pick one."
                        .to_string(),
                );
                return;
            }
            let Some(file) = state.selected_diff_file() else {
                return;
            };
            let starts = file.hunk_start_indices();
            let Some(index) = hunk_index_for_line(&starts, state.cursor_line) else {
                return;
            };
            let span = hunk_span(file, index);
            state.error = None;
            state.anchor = LearningAnchor::Hunk { index };
            if let Some((start, end)) = span {
                state.cursor_line = start;
                state.selection_anchor = Some(end);
            }
        }
    }

    /// The text the current anchor covers, captured verbatim onto a Q&A row so
    /// the answer stays readable after the file moves on.
    pub fn learning_selection_text(&self) -> String {
        match &self.mode {
            AppMode::Learning(state) => selection_text(state),
            _ => String::new(),
        }
    }
}

// ── pure helpers (unit-tested) ───────────────────────────────

/// Which `START_HERE_CANDIDATES` actually exist in `workdir`, in candidate
/// order. Directories don't count — the group is a reading list.
pub fn start_here_candidates(workdir: &Path) -> Vec<String> {
    START_HERE_CANDIDATES
        .iter()
        .filter(|candidate| workdir.join(candidate).is_file())
        .map(|candidate| (*candidate).to_string())
        .collect()
}

/// The repo-tree file list: the pinned orientation group (when it has any
/// members and hasn't been collapsed), then every file.
pub fn build_repo_tree_entries(
    files: &[String],
    start_here: &[String],
    collapsed: bool,
) -> Vec<LearningListEntry> {
    let mut entries = Vec::with_capacity(files.len() + start_here.len() + 2);
    if !start_here.is_empty() {
        entries.push(LearningListEntry::StartHereHeader);
        if !collapsed {
            entries.push(LearningListEntry::ProjectTour);
            for path in start_here {
                entries.push(LearningListEntry::File {
                    path: path.clone(),
                    group: LearningListGroup::StartHere,
                    diff_index: None,
                });
            }
        }
    }
    for path in files {
        entries.push(LearningListEntry::File {
            path: path.clone(),
            group: LearningListGroup::Files,
            diff_index: None,
        });
    }
    entries
}

/// The branch-changes file list. No orientation group here: the user already
/// knows what they're looking for when they're reading their own diff.
pub fn build_changed_entries(files: &[DiffFile]) -> Vec<LearningListEntry> {
    files
        .iter()
        .enumerate()
        .map(|(i, file)| LearningListEntry::File {
            path: file.path.clone(),
            group: LearningListGroup::Files,
            diff_index: Some(i),
        })
        .collect()
}

/// The whole file a diff entry covers, current side where there is one. A
/// deletion only has a base side, and a binary file has neither — an empty
/// result simply means "no surrounding file to offer".
pub fn diff_file_lines(file: &DiffFile) -> Vec<String> {
    file.new_content
        .as_deref()
        .or(file.old_content.as_deref())
        .map(|text| text.lines().map(ToOwned::to_owned).collect())
        .unwrap_or_default()
}

/// Read a file for the content pane, or say why it can't be shown. The message
/// is user-facing, so it names the limit rather than the errno.
pub fn load_file_lines(path: &Path) -> Result<Vec<String>, String> {
    let meta =
        std::fs::metadata(path).map_err(|e| format!("Couldn't open {}: {e}", path.display()))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "This file is {} — too big to show here (the limit is {} MB).",
            human_bytes(meta.len()),
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    let bytes =
        std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    if looks_binary(&bytes) {
        return Err("This looks like a binary file, so there's nothing to read here.".to_string());
    }
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.lines().map(ToOwned::to_owned).collect())
}

/// A NUL byte in the first few KB is the same heuristic git uses.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0)
}

fn human_bytes(len: u64) -> String {
    if len >= 1024 * 1024 {
        format!("{:.1} MB", len as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", len as f64 / 1024.0)
    }
}

/// Depth- and entry-capped walk for projects git doesn't know about. There are
/// no ignore rules to inherit here, so [`WALK_SKIP_DIRS`] stands in for them.
pub fn walk_files_capped(root: &Path, max_entries: usize, max_depth: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= max_entries || depth > max_depth {
            continue;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if WALK_SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push((path, depth + 1));
            } else if path.is_file() {
                if out.len() >= max_entries {
                    break;
                }
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    out.sort();
    out.truncate(max_entries);
    out
}

/// Trim a repo-tree listing to `max_entries`, returning the original count
/// when anything was dropped so the caller can say so. A monorepo browsed by
/// someone unfamiliar with it is the worst case for both listing cost and
/// usefulness, and silently showing a partial list would read as a bug.
pub fn cap_repo_entries(files: &mut Vec<String>, max_entries: usize) -> Option<usize> {
    let total = files.len();
    if total <= max_entries {
        return None;
    }
    files.truncate(max_entries);
    Some(total)
}

/// The anchor implied by the content cursor and any in-progress range: a
/// 1-based, inclusive line range in the file.
pub fn anchor_for_cursor(state: &LearningViewState) -> LearningAnchor {
    let (from, to) = state.selected_span();
    match state.scope {
        BrowseScope::RepoTree => {
            if state.content.is_empty() {
                return LearningAnchor::File;
            }
            let last = state.content.len() - 1;
            LearningAnchor::Lines {
                start: from.min(last) + 1,
                end: to.min(last) + 1,
            }
        }
        BrowseScope::BranchChanges => {
            let Some(file) = state.selected_diff_file() else {
                return LearningAnchor::File;
            };
            let lines = file.addressable_lines();
            if lines.is_empty() {
                return LearningAnchor::File;
            }
            let start = lines
                .get(from.min(lines.len() - 1))
                .and_then(diff_line_number);
            let end = lines
                .get(to.min(lines.len() - 1))
                .and_then(diff_line_number);
            match (start, end) {
                (Some(start), Some(end)) => LearningAnchor::Lines {
                    start: start.min(end),
                    end: start.max(end),
                },
                // A pure-deletion span has no current-side line to point at;
                // fall back to the file rather than inventing a number.
                _ => LearningAnchor::File,
            }
        }
    }
}

/// The line number a diff row points at: its current-side number, or its
/// base-side number for a removed line.
fn diff_line_number(loc: &DiffLineLocation) -> Option<usize> {
    loc.new_line.or(loc.old_line)
}

/// The hunk containing addressable-line index `line`, given each hunk's start.
pub fn hunk_index_for_line(hunk_starts: &[usize], line: usize) -> Option<usize> {
    hunk_starts
        .iter()
        .rposition(|start| *start <= line)
        .or(if hunk_starts.is_empty() {
            None
        } else {
            Some(0)
        })
}

/// The `(first, last)` addressable-line indices of hunk `index`.
fn hunk_span(file: &DiffFile, index: usize) -> Option<(usize, usize)> {
    let starts = file.hunk_start_indices();
    let start = *starts.get(index)?;
    let end = starts
        .get(index + 1)
        .map(|next| next.saturating_sub(1))
        .unwrap_or_else(|| file.addressable_lines().len().saturating_sub(1));
    Some((start, end.max(start)))
}

/// The text covered by `state.anchor`.
///
/// A file anchor always yields the whole file, in either scope — that is what
/// the anchor promises. A hunk or line anchor in branch-changes scope yields
/// *diff* rows, markers included, because that is what the user selected;
/// [`LearningViewState::selection_is_diff`] tells the prompt builder to label
/// it as such.
pub fn selection_text(state: &LearningViewState) -> String {
    match state.anchor {
        // The project anchor has no text: the question is about the repo.
        LearningAnchor::Project => String::new(),
        // `content` is the file on disk in repo-tree scope and the snapshot's
        // copy of it in branch-changes scope, so the whole file either way.
        LearningAnchor::File => state.content.join("\n"),
        LearningAnchor::Hunk { index } => {
            let Some(file) = state.selected_diff_file() else {
                return String::new();
            };
            let texts = file.addressable_line_diff_texts();
            match hunk_span(file, index) {
                Some((start, end)) => texts
                    .get(start..=end.min(texts.len().saturating_sub(1)))
                    .map(|slice| slice.join("\n"))
                    .unwrap_or_default(),
                None => String::new(),
            }
        }
        LearningAnchor::Lines { .. } => {
            let (from, to) = state.selected_span();
            match state.scope {
                BrowseScope::RepoTree => state
                    .content
                    .get(from..=to.min(state.content.len().saturating_sub(1)))
                    .map(|slice| slice.join("\n"))
                    .unwrap_or_default(),
                BrowseScope::BranchChanges => {
                    let Some(file) = state.selected_diff_file() else {
                        return String::new();
                    };
                    let texts = file.addressable_line_diff_texts();
                    texts
                        .get(from..=to.min(texts.len().saturating_sub(1)))
                        .map(|slice| slice.join("\n"))
                        .unwrap_or_default()
                }
            }
        }
    }
}

// ── prompt building ──────────────────────────────────────────

/// Lines of surrounding file shown either side of the selection. Enough for
/// the agent to see what the selection sits inside without carrying a whole
/// large file into a no-tools prompt.
const CONTEXT_WINDOW_LINES: usize = 80;

/// Hard cap on the surrounding-context block.
const MAX_CONTEXT_LINES: usize = 400;

/// Hard cap on the quoted selection. A whole-file anchor on a big file would
/// otherwise blow the prompt up on its own.
const MAX_SELECTION_LINES: usize = 400;

/// How many ancestors of a follow-up are carried into its prompt. Deeper
/// ancestors are dropped oldest-first, which bounds prompt growth at the cost
/// of context a later question might have depended on (see the plan's
/// "follow-up threading grows prompts").
pub const MAX_FOLLOW_UP_DEPTH: usize = 3;

/// One earlier turn carried into a follow-up prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentTurn {
    pub question: String,
    pub answer: String,
}

/// Everything a prompt needs to know about where a question came from. Built
/// from the overlay state, but a plain value so the builders stay pure and
/// testable.
#[derive(Debug, Clone)]
pub struct LearningPromptContext {
    pub project_name: String,
    pub feature_name: String,
    /// Repo-relative path, `None` for the project anchor.
    pub file_path: Option<String>,
    pub anchor: LearningAnchor,
    /// The text the anchor covers.
    pub selection_text: String,
    /// Whether [`selection_text`](Self::selection_text) is a unified-diff
    /// excerpt. The block is presented differently when it is: markers
    /// explained, and no line numbers, since removed lines have none on the
    /// current side.
    pub selection_is_diff: bool,
    /// The whole file the selection came from, for surrounding context.
    pub file_lines: Vec<String>,
    /// 1-based line the selection starts at, when it has one.
    pub selection_start_line: Option<usize>,
    pub question: String,
    pub intent: LearningQaIntent,
    pub level: LearningLevel,
    /// What the run may look at. Set by `learning_enqueue` from the mode that
    /// will actually be dispatched, so the prompt and the row's label can't
    /// disagree about whether the repository was read.
    pub run_mode: crate::app::LearningRunMode,
    /// Oldest first. Trimmed to [`MAX_FOLLOW_UP_DEPTH`] by the builder.
    pub ancestors: Vec<ParentTurn>,
}

/// Build the prompt for one question.
///
/// Structure is fixed: who and where, then what they're looking at, then any
/// earlier turns, then the question, then the instructions selected by intent,
/// level, and run mode. Instructions come last so they're the freshest thing
/// the model reads, and the run mode last of all — what may be checked, and
/// what must be, outranks how the answer is worded.
pub fn build_prompt(ctx: &LearningPromptContext) -> String {
    let mut out = String::new();

    out.push_str("You are helping someone read a codebase they did not write.\n\n");
    out.push_str(&format!("Project: {}\n", ctx.project_name));
    out.push_str(&format!("Branch / feature: {}\n", ctx.feature_name));
    if let Some(path) = &ctx.file_path {
        out.push_str(&format!("File: {path}\n"));
    }
    out.push_str(&format!(
        "They are looking at: {}\n\n",
        ctx.anchor.describe(ctx.file_path.as_deref())
    ));

    if matches!(ctx.anchor, LearningAnchor::Project) {
        out.push_str("Their question is about the project as a whole, not about one file.\n\n");
    } else if !ctx.selection_text.trim().is_empty() {
        if ctx.selection_is_diff {
            // Unnumbered on purpose: these rows come from a unified diff, where
            // a removed line has no number on the current side and the numbers
            // that do exist are not consecutive.
            out.push_str(
                "--- The change they are asking about (unified diff — lines starting \
                 with '+' were added, '-' were removed, ' ' are unchanged context) ---\n",
            );
            out.push_str(&plain_block(&ctx.selection_text, MAX_SELECTION_LINES));
        } else {
            out.push_str("--- The code they are asking about ---\n");
            out.push_str(&numbered_block(
                &ctx.selection_text,
                ctx.selection_start_line.unwrap_or(1),
                MAX_SELECTION_LINES,
            ));
        }
        out.push_str("\n\n");
    }

    if let Some(context) = surrounding_context(ctx) {
        out.push_str(&context);
        out.push_str("\n\n");
    }

    let ancestors = trimmed_ancestors(&ctx.ancestors);
    if !ancestors.is_empty() {
        out.push_str("--- Earlier in this conversation ---\n");
        for turn in ancestors {
            out.push_str(&format!("They asked: {}\n", turn.question.trim()));
            out.push_str(&format!("You answered: {}\n\n", turn.answer.trim()));
        }
    }

    out.push_str("--- Their question ---\n");
    out.push_str(ctx.question.trim());
    out.push_str("\n\n");

    out.push_str(intent_instructions(ctx.intent));
    out.push('\n');
    out.push_str(level_instructions(ctx.level));
    out.push('\n');
    out.push_str(run_mode_instructions(ctx.run_mode));
    out
}

/// What the run may look at — and, for a deep dive, what it is obliged to do
/// with that access.
///
/// The row and the answer pane label a deep dive "read the repo", and the whole
/// point of the action is catching a first answer that invented a file or a
/// line number. Read-only tools only make that possible; without being told to,
/// an agent can answer straight from the excerpt and the claim on the row
/// becomes false. So the deep-dive text requires the reading and requires the
/// answer to name what was read, which is also what makes the two answers
/// comparable. The no-tools text is the mirror image: say what you cannot see
/// rather than filling it in.
pub fn run_mode_instructions(mode: crate::app::LearningRunMode) -> &'static str {
    match mode {
        crate::app::LearningRunMode::NoTools => {
            "You are answering from what is quoted above and nothing else — you \
             have no access to the rest of the repository, and you must not \
             claim otherwise. Do not invent file paths, symbols, line numbers, \
             or command output you cannot see here. Where the answer depends on \
             code that is not shown, say so plainly and name the file you would \
             need to read.\n"
        }
        crate::app::LearningRunMode::DeepDive => {
            "You have read-only access to this repository, and this answer is \
             shown to them as one that read it — so read it. Before you answer, \
             open the file above and whatever it depends on: the definitions it \
             calls, the places that call it, and any test that exercises it. \
             Ground every claim in what you actually read, and name the files \
             and symbols you checked so they can follow you. If the code \
             contradicts what you would otherwise have assumed, say so \
             explicitly. If you looked for something and could not find it, say \
             that rather than guessing.\n"
        }
    }
}

/// What the answer is for. This is the only place intent changes anything
/// about the run.
pub fn intent_instructions(intent: LearningQaIntent) -> &'static str {
    match intent {
        LearningQaIntent::Explain => {
            "Explain what this code does and why it is written this way. \
             Answer the question they actually asked. \
             Do not propose changes, rewrites, or improvements — they asked to \
             understand this code, not to change it. If something looks wrong, \
             you may say so in one sentence, but do not turn the answer into a \
             proposal.\n"
        }
        LearningQaIntent::Action => {
            "Propose the smallest concrete change that satisfies their request. \
             Begin your answer with a single line that is an imperative summary \
             of the change, under 80 characters, with no markdown formatting and \
             no trailing period — it is used verbatim as the title of a work \
             item. Then explain what to change, where, and why it is worth \
             changing. Do not make the change yourself; describe it.\n"
        }
    }
}

/// How much the answer may assume. Prompt wording only — it changes no tools,
/// no model, and nothing about which files are visible.
pub fn level_instructions(level: LearningLevel) -> &'static str {
    match level {
        LearningLevel::Newcomer => {
            "Write for someone who has never seen this codebase and may be new \
             to the language. Define every technical term the first time you use \
             it. Prefer short paragraphs and concrete examples over abstraction. \
             Do not assume they know this project's own vocabulary. No question \
             is too basic — answer it plainly rather than commenting on how basic \
             it is. Finish with a section headed \"Where to look next\" listing \
             specific files or symbols and one line on why each is worth \
             reading.\n"
        }
        LearningLevel::Familiar => {
            "Write for someone comfortable in this language who is new only to \
             this codebase. Be dense and skip the basics: no glossary, no \
             definitions of standard language features, and no \"where to look \
             next\" section.\n"
        }
    }
}

/// The most recent [`MAX_FOLLOW_UP_DEPTH`] turns, oldest first.
fn trimmed_ancestors(ancestors: &[ParentTurn]) -> &[ParentTurn] {
    let start = ancestors.len().saturating_sub(MAX_FOLLOW_UP_DEPTH);
    &ancestors[start..]
}

/// The file around the selection, line-numbered. Skipped when the anchor is
/// the whole file (the selection *is* the file) or the project.
fn surrounding_context(ctx: &LearningPromptContext) -> Option<String> {
    if ctx.file_lines.is_empty() {
        return None;
    }
    if matches!(ctx.anchor, LearningAnchor::Project | LearningAnchor::File) {
        return None;
    }
    let path = ctx.file_path.as_deref().unwrap_or("the file");
    let selection_start = ctx.selection_start_line.unwrap_or(1).max(1);
    let first = selection_start.saturating_sub(CONTEXT_WINDOW_LINES).max(1);
    let selection_lines = ctx.selection_text.lines().count().max(1);
    let last = (selection_start + selection_lines + CONTEXT_WINDOW_LINES)
        .min(ctx.file_lines.len())
        .min(first + MAX_CONTEXT_LINES);
    if last < first {
        return None;
    }
    let block: Vec<String> = ctx.file_lines[first - 1..last].to_vec();
    Some(format!(
        "--- Surrounding context: {path}, lines {first}-{last} ---\n{}",
        numbered_block(&block.join("\n"), first, MAX_CONTEXT_LINES)
    ))
}

/// Text carried through verbatim, under the same truncation rule as
/// [`numbered_block`]. For excerpts whose own leading characters are the
/// point — a diff — where a line-number gutter would only be misleading.
fn plain_block(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = lines
        .iter()
        .take(max_lines)
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    if lines.len() > max_lines {
        out.push_str(&format!(
            "… {} more lines not shown\n",
            lines.len() - max_lines
        ));
    }
    // Trailing newline is added by the caller's separator.
    out.pop();
    out
}

/// Line-numbered text, truncated with an explicit marker so the model can tell
/// truncation from the real end of a file.
fn numbered_block(text: &str, start_line: usize, max_lines: usize) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().take(max_lines).enumerate() {
        out.push_str(&format!("{:>6} | {line}\n", start_line + i));
    }
    if lines.len() > max_lines {
        out.push_str(&format!(
            "       … {} more lines not shown\n",
            lines.len() - max_lines
        ));
    }
    // Trailing newline is added by the caller's separator.
    out.pop();
    out
}

/// The place a question is asked about, captured so it can't move under the
/// user. A follow-up reuses its parent's capture verbatim, which is why these
/// three fields are persisted on every row.
#[derive(Debug, Clone)]
pub struct AskAnchor {
    pub anchor: LearningAnchor,
    pub file_path: Option<String>,
    pub selection_text: String,
    /// Captured with the text, not re-read at submit time: browsing away from
    /// branch-changes scope must not turn a quoted diff into numbered source.
    pub selection_is_diff: bool,
}

impl App {
    /// Assemble the prompt context for a question asked right now, against the
    /// overlay's current anchor.
    pub fn learning_prompt_context(
        &self,
        question: &str,
        intent: LearningQaIntent,
        ancestors: Vec<ParentTurn>,
    ) -> Option<LearningPromptContext> {
        self.learning_prompt_context_at(question, intent, ancestors, None)
    }

    /// As above, but against `captured` when a question inherits its place
    /// from somewhere other than the cursor — a follow-up asked from the
    /// answer pane, where the file list may have moved on since.
    pub fn learning_prompt_context_at(
        &self,
        question: &str,
        intent: LearningQaIntent,
        ancestors: Vec<ParentTurn>,
        captured: Option<&AskAnchor>,
    ) -> Option<LearningPromptContext> {
        let AppMode::Learning(state) = &self.mode else {
            return None;
        };
        let anchor = captured.map(|c| c.anchor).unwrap_or(state.anchor);
        let file_path = match anchor {
            LearningAnchor::Project => None,
            _ => match captured {
                Some(c) => c.file_path.clone(),
                None => state.content_path.clone(),
            },
        };
        let selection_start_line = match anchor {
            LearningAnchor::Lines { start, .. } => Some(start),
            LearningAnchor::Hunk { .. } => match anchor_for_cursor(state) {
                LearningAnchor::Lines { start, .. } => Some(start),
                _ => None,
            },
            _ => Some(1),
        };
        // Surrounding context is only honest while the loaded file is still
        // the one being asked about; a follow-up on a file the user has since
        // browsed away from gets its parent's turn instead of the wrong file.
        let file_lines = if file_path.is_some() && file_path == state.content_path {
            state.content.clone()
        } else if captured.is_some() {
            Vec::new()
        } else {
            state.content.clone()
        };
        Some(LearningPromptContext {
            project_name: state.project_name.clone(),
            feature_name: state.feature_name.clone(),
            file_path,
            anchor,
            selection_text: match captured {
                Some(c) => c.selection_text.clone(),
                None => selection_text(state),
            },
            selection_is_diff: match captured {
                Some(c) => c.selection_is_diff,
                None => state.selection_is_diff(),
            },
            file_lines,
            selection_start_line,
            question: question.to_string(),
            intent,
            level: state.level,
            // Provisional: `learning_enqueue` overwrites it with the mode that
            // is actually dispatched, once `effective_for` has had its say.
            run_mode: crate::app::LearningRunMode::NoTools,
            ancestors,
        })
    }
}

// ── asking (headless, non-blocking) ──────────────────────────

/// Where a new follow-up on `parent_id` belongs: just past the parent and
/// everything already hanging off it, so a thread stays contiguous.
///
/// `None` when the parent isn't in `rows` — a stale id appends rather than
/// disappearing.
pub fn thread_insert_index(rows: &[LearningQa], parent_id: &str) -> Option<usize> {
    let mut last = rows.iter().position(|row| row.id == parent_id)?;
    let mut thread: Vec<&str> = vec![parent_id];
    // One pass per row is enough: rows are stored parent-before-child, so a
    // descendant is always seen after the ancestor that admits it.
    for (index, row) in rows.iter().enumerate().skip(last + 1) {
        let parent = row.parent_qa_id.as_deref();
        if parent.is_some_and(|p| thread.contains(&p)) {
            thread.push(&row.id);
            last = index;
        }
    }
    Some(last + 1)
}

/// A finished headless run, delivered back to the UI thread.
pub struct LearningAnswer {
    /// `learning_qa.id` the answer belongs to.
    pub qa_id: String,
    /// `Ok(answer)` or a message phrased as what to do about it.
    pub result: Result<String, String>,
}

impl App {
    /// Enqueue a question against the overlay's current anchor and return
    /// immediately. The run happens on its own thread; the row shows "queued"
    /// then "thinking…" until [`App::poll_learning_answers_bg`] files the
    /// answer. Several questions may be in flight at once.
    ///
    /// Returns the new row's id.
    pub fn learning_ask(
        &mut self,
        question: &str,
        intent: LearningQaIntent,
        parent_qa_id: Option<String>,
    ) -> Option<String> {
        self.learning_ask_at(question, intent, parent_qa_id, None)
    }

    /// As above, against an explicitly captured place rather than wherever the
    /// cursor happens to be. A follow-up passes its parent's capture, so it
    /// asks about the same code even if the file list has moved on.
    pub fn learning_ask_at(
        &mut self,
        question: &str,
        intent: LearningQaIntent,
        parent_qa_id: Option<String>,
        captured: Option<AskAnchor>,
    ) -> Option<String> {
        let question = question.trim().to_string();
        if question.is_empty() {
            return None;
        }
        let ancestors = self.learning_ancestor_turns(parent_qa_id.as_deref());
        let ctx =
            self.learning_prompt_context_at(&question, intent, ancestors, captured.as_ref())?;
        self.learning_enqueue(
            ctx,
            parent_qa_id,
            crate::app::LearningRunMode::NoTools,
            None,
        )
    }

    /// Write a row for `ctx` and start its run. The single place a `learning_qa`
    /// row is born, so asking and re-asking can't drift apart.
    ///
    /// `deep_dive_of` is set only by [`App::learning_deep_dive`], and always to
    /// the same row as `parent_qa_id`: the pair threads together but does not
    /// converse (see [`LearningQa::deep_dive_of`]).
    fn learning_enqueue(
        &mut self,
        mut ctx: LearningPromptContext,
        parent_qa_id: Option<String>,
        run_mode: crate::app::LearningRunMode,
        deep_dive_of: Option<String>,
    ) -> Option<String> {
        let AppMode::Learning(state) = &mut self.mode else {
            return None;
        };
        // Recorded as what will actually run: a harness with no no-tools mode
        // is a deep dive whatever was asked for (see `effective_for`).
        let run_mode = run_mode.effective_for(&state.harness);
        // The prompt is written for the run that will happen, not the one that
        // was asked for, so a downgraded row can't be told to answer from the
        // excerpt alone while its label says it read the repository.
        ctx.run_mode = run_mode;
        let qa = LearningQa {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: state.session_id.clone(),
            parent_qa_id: parent_qa_id.clone(),
            deep_dive_of,
            file_path: ctx.file_path.clone(),
            anchor: ctx.anchor,
            selection_text: ctx.selection_text.clone(),
            selection_is_diff: ctx.selection_is_diff,
            question: ctx.question.clone(),
            intent: ctx.intent,
            // From the context, not the live setting: a re-run preserves the
            // level its original was answered at, so the pair reads alike.
            level: ctx.level,
            answer: None,
            harness: state.harness.clone(),
            run_mode,
            status: crate::app::LearningQaStatus::Pending,
            error: None,
            todo_id: None,
            spawned_session_id: None,
            created_at: crate::db::learning::now_timestamp(),
            updated_at: crate::db::learning::now_timestamp(),
        };
        let qa_id = qa.id.clone();
        let harness = qa.harness.clone();
        let workdir = state.workdir.clone();
        // A follow-up belongs under the thread it continues, not at the bottom
        // of the history — the renderer indents it under its parent, and a row
        // indented under something twenty rows above it reads as a glitch.
        let at = parent_qa_id
            .as_deref()
            .and_then(|parent| thread_insert_index(&state.qa, parent))
            .unwrap_or(state.qa.len());
        state.qa.insert(at, qa.clone());
        // Show the new question, so an answer that takes a while is visibly
        // *this* question's answer.
        state.selected_qa = at;

        self.persist_learning_qa(&qa);
        self.spawn_learning_run(&qa_id, harness, workdir, build_prompt(&ctx), run_mode);
        Some(qa_id)
    }

    /// Re-ask the selected question with the repository open to the agent.
    ///
    /// The first answer comes from a no-tools run that can only see the prompt,
    /// so it can name files, symbols, and line numbers that do not exist — the
    /// failure a newcomer is least equipped to spot. A deep dive is the answer
    /// to that: same question, same anchor, same intent and reading level, run
    /// through [`HeadlessRunner::run_read_only`] in the feature's workdir so the
    /// agent can go and check.
    ///
    /// It lands as its own row indented under the original, and the original
    /// answer is left untouched so the two can be read against each other. The
    /// original answer is deliberately **not** fed into the prompt: a rerun that
    /// re-derives the facts is worth more than one anchored on a guess. The new
    /// row records the original in
    /// [`deep_dive_of`](crate::app::LearningQa::deep_dive_of) as well as in
    /// `parent_qa_id`, which is what keeps that answer out of *later* prompts
    /// too — a follow-up on the deep dive continues from the deep dive.
    ///
    /// Returns the new row's id, or `None` when nothing was started (the banner
    /// says why).
    ///
    /// [`HeadlessRunner::run_read_only`]: crate::headless::HeadlessRunner::run_read_only
    pub fn learning_deep_dive(&mut self) -> Option<String> {
        let Some(origin) = (match &self.mode {
            AppMode::Learning(state) => state.qa.get(state.selected_qa).cloned(),
            _ => return None,
        }) else {
            self.learning_error("Ask something first — a deep dive re-runs a question you already asked, letting the agent read the repo.");
            return None;
        };
        // Checked before the in-flight guard: a row that reads the repository
        // is refused whether or not it has landed, so telling the user to wait
        // for it would be promising something that is then refused. Also the
        // Codex case — `effective_for` already downgraded that row to a deep
        // dive, so there is genuinely nothing deeper to go.
        if origin.run_mode == crate::app::LearningRunMode::DeepDive {
            self.learning_error(if origin.status.is_in_flight() {
                "That one is already reading the repository. Once it lands, ask a follow-up (F) to go further."
            } else {
                "That answer already read the repository. Ask a follow-up (F) to go further."
            });
            return None;
        }
        if origin.status.is_in_flight() {
            self.learning_error(
                "That answer is still generating — you can send it deeper once it arrives.",
            );
            return None;
        }
        // One deep dive per question: a second identical run costs the same and
        // says the same thing, so jump to the one that exists instead.
        //
        // Matched on `deep_dive_of` rather than parent + run mode, which would
        // mistake an ordinary follow-up for a deep dive under Codex, where
        // every row is recorded as one.
        let existing = match &self.mode {
            AppMode::Learning(state) => state
                .qa
                .iter()
                .position(|row| {
                    row.deep_dive_of.as_deref() == Some(origin.id.as_str())
                        && row.status != crate::app::LearningQaStatus::Failed
                })
                .map(|index| (index, state.qa[index].status.is_in_flight())),
            _ => None,
        };
        if let Some((index, in_flight)) = existing {
            if let AppMode::Learning(state) = &mut self.mode {
                state.selected_qa = index;
                state.answer_open = false;
                // An unfinished run has nothing to show yet, so it must not be
                // described as something that came back.
                state.error = Some(
                    if in_flight {
                        "You already sent that one deeper — it is still reading the repository."
                    } else {
                        "You already sent that one deeper — here is what it came back with."
                    }
                    .into(),
                );
            }
            return None;
        }

        let ctx = self.learning_deep_dive_context(&origin)?;
        if let AppMode::Learning(state) = &mut self.mode {
            state.error = None;
            state.answer_open = false;
        }
        self.learning_enqueue(
            ctx,
            Some(origin.id.clone()),
            crate::app::LearningRunMode::DeepDive,
            Some(origin.id.clone()),
        )
    }

    /// The prompt a deep dive of `origin` would send.
    ///
    /// Everything comes off the row rather than off the live overlay, so a
    /// question sent deeper after browsing elsewhere still asks about its own
    /// code at its own reading level.
    pub(crate) fn learning_deep_dive_context(
        &self,
        origin: &LearningQa,
    ) -> Option<LearningPromptContext> {
        let captured = AskAnchor {
            anchor: origin.anchor,
            file_path: origin.file_path.clone(),
            selection_text: origin.selection_text.clone(),
            selection_is_diff: origin.selection_is_diff,
        };
        // The conversation that led *to* the origin, not including the origin —
        // a deep dive occupies the origin's position in the thread rather than
        // continuing past it, which is what keeps the answer it is checking out
        // of the prompt that checks it.
        let ancestors = self.learning_ancestor_turns(origin.parent_qa_id.as_deref());
        let mut ctx = self.learning_prompt_context_at(
            &origin.question,
            origin.intent,
            ancestors,
            Some(&captured),
        )?;
        ctx.level = origin.level;
        ctx.run_mode = crate::app::LearningRunMode::DeepDive;
        Some(ctx)
    }

    /// Set the overlay's banner — the "why nothing happened" channel.
    fn learning_error(&mut self, message: impl Into<String>) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.error = Some(message.into());
        }
    }

    /// Start the headless run for an existing row and mark it running.
    pub fn spawn_learning_run(
        &mut self,
        qa_id: &str,
        harness: AgentKind,
        workdir: PathBuf,
        prompt: String,
        run_mode: crate::app::LearningRunMode,
    ) {
        // The dispatch below and the mode stored on the row must agree, so both
        // go through `effective_for` rather than trusting the caller's ask.
        let run_mode = run_mode.effective_for(&harness);
        let tx = self.learning_answer_tx.clone();
        let id = qa_id.to_string();
        self.log_info(
            "learning",
            format!(
                "asking {} ({}) about {} chars of context",
                harness.display_name(),
                run_mode.as_str(),
                prompt.len()
            ),
        );
        // Remembered so a reopen of this overlay doesn't mistake a run this
        // process is still waiting on for one stranded by an earlier one (see
        // `reconcile_interrupted_qa`).
        self.learning_runs_in_flight.insert(id.clone());
        // Tests drive the same channel by hand (see `deliver`). Launching a
        // real agent CLI from a unit test would be slow, flaky, and would spend
        // the developer's tokens, so the row still transitions to Running but
        // no process is started.
        if cfg!(test) {
            self.set_learning_qa_status(qa_id, crate::app::LearningQaStatus::Running, None);
            return;
        }
        std::thread::spawn(move || {
            let result = match run_mode {
                crate::app::LearningRunMode::NoTools => {
                    crate::headless::HeadlessRunner::run(&harness, &workdir, &prompt, None, true)
                }
                crate::app::LearningRunMode::DeepDive => {
                    crate::headless::HeadlessRunner::run_read_only(
                        &harness, &workdir, &prompt, None,
                    )
                }
            };
            let _ = tx.send(LearningAnswer {
                qa_id: id,
                result: result.map_err(|e| headless_failure_message(&harness, &e)),
            });
        });
        self.set_learning_qa_status(qa_id, crate::app::LearningQaStatus::Running, None);
    }

    /// Drain finished answers. Called from the main loop beside the other
    /// `poll_*_bg` calls; returns true when something changed and the UI
    /// should redraw.
    pub fn poll_learning_answers_bg(&mut self) -> bool {
        let mut changed = false;
        while let Ok(answer) = self.learning_answer_rx.try_recv() {
            changed = true;
            self.learning_runs_in_flight.remove(&answer.qa_id);
            let outcome = match answer.result {
                Ok(text) => Ok(text.trim().to_string()),
                Err(message) => {
                    self.log_error("learning", format!("question failed: {message}"));
                    Err(message)
                }
            };
            let mut applied = false;
            if let AppMode::Learning(state) = &mut self.mode
                && let Some(row) = state.qa.iter_mut().find(|r| r.id == answer.qa_id)
            {
                match &outcome {
                    Ok(text) => {
                        row.answer = Some(text.clone());
                        row.status = crate::app::LearningQaStatus::Answered;
                        row.error = None;
                    }
                    // A failure leaves any earlier answer in place: a rerun
                    // that couldn't start is no reason to lose what the first
                    // run already said.
                    Err(message) => {
                        row.status = crate::app::LearningQaStatus::Failed;
                        row.error = Some(message.clone());
                    }
                }
                row.updated_at = crate::db::learning::now_timestamp();
                applied = true;
            }
            if applied {
                self.persist_learning_qa_by_id(&answer.qa_id);
            } else {
                self.finish_learning_qa_in_db(&answer.qa_id, &outcome);
            }
        }
        changed
    }

    /// Complete a row straight in the DB, for a run that finished after the
    /// overlay that started it closed or moved to another project.
    ///
    /// The in-memory row is the overlay's source of truth, so once the overlay
    /// is gone there is nothing for `persist_learning_qa` to write and the row
    /// would sit at `running` in the database for good — reopening the session
    /// would show a question that never finishes.
    fn finish_learning_qa_in_db(&mut self, qa_id: &str, outcome: &Result<String, String>) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let result = match outcome {
            Ok(text) => db.finish_learning_qa(
                qa_id,
                Some(text),
                crate::app::LearningQaStatus::Answered,
                None,
            ),
            Err(message) => db.finish_learning_qa(
                qa_id,
                None,
                crate::app::LearningQaStatus::Failed,
                Some(message),
            ),
        };
        match result {
            Ok(true) => self.log_info(
                "learning",
                format!("saved an answer that finished after its overlay closed ({qa_id})"),
            ),
            Ok(false) => self.log_warn(
                "learning",
                format!("an answer arrived for a question that no longer exists ({qa_id})"),
            ),
            Err(e) => self.log_warn(
                "learning",
                format!("couldn't save an answer that finished after its overlay closed: {e}"),
            ),
        }
    }

    /// The chain of earlier turns leading to `parent_qa_id`, oldest first.
    /// Only answered rows are carried — an unanswered parent has no context to
    /// give.
    ///
    /// A deep dive in the chain is followed *through* the row it re-ran rather
    /// than into it: it stands in that row's place, so the answer it was run to
    /// check — the one that may have invented files and line numbers — never
    /// re-enters a later prompt through the back door.
    fn learning_ancestor_turns(&self, parent_qa_id: Option<&str>) -> Vec<ParentTurn> {
        let AppMode::Learning(state) = &self.mode else {
            return Vec::new();
        };
        let mut chain = Vec::new();
        let mut current = parent_qa_id.map(str::to_string);
        // Bounded by the row count, so a cycle in the data can't hang the UI.
        for _ in 0..state.qa.len() {
            let Some(id) = current.take() else { break };
            let Some(row) = state.qa.iter().find(|r| r.id == id) else {
                break;
            };
            if let Some(answer) = &row.answer {
                chain.push(ParentTurn {
                    question: row.question.clone(),
                    answer: answer.clone(),
                });
            }
            current = match row.superseded_id() {
                // Skip the superseded row and resume above it. Its own parent
                // is where the conversation actually continues.
                Some(superseded) => state
                    .qa
                    .iter()
                    .find(|r| r.id == superseded)
                    .and_then(|r| r.parent_qa_id.clone()),
                None => row.parent_qa_id.clone(),
            };
        }
        chain.reverse();
        chain
    }

    fn set_learning_qa_status(
        &mut self,
        qa_id: &str,
        status: crate::app::LearningQaStatus,
        error: Option<String>,
    ) {
        if let AppMode::Learning(state) = &mut self.mode
            && let Some(row) = state.qa.iter_mut().find(|r| r.id == qa_id)
        {
            row.status = status;
            row.error = error;
            row.updated_at = crate::db::learning::now_timestamp();
        }
        self.persist_learning_qa_by_id(qa_id);
    }

    fn persist_learning_qa_by_id(&mut self, qa_id: &str) {
        let row = match &self.mode {
            AppMode::Learning(state) => state.qa.iter().find(|r| r.id == qa_id).cloned(),
            _ => None,
        };
        if let Some(row) = row {
            self.persist_learning_qa(&row);
        }
    }

    /// Write a row through to the DB when there is one. History surviving a
    /// restart is a nice-to-have here, not a precondition, so a failure is
    /// logged and the in-memory row carries on.
    pub fn persist_learning_qa(&mut self, qa: &LearningQa) {
        if qa.session_id.is_empty() {
            return;
        }
        let Some(db) = self.db.as_ref() else { return };
        if let Err(e) = db.upsert_learning_qa(qa) {
            self.log_warn(
                "learning",
                format!("couldn't save this question: {e} (it still works in this session)"),
            );
        }
    }
}

// ── panes, history cursor, answer view ───────────────────────

impl App {
    /// Move focus file list → content → history → file list.
    pub fn learning_cycle_focus(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.focus = match state.focus {
                LearningFocus::FileList => LearningFocus::Content,
                LearningFocus::Content => LearningFocus::Qa,
                LearningFocus::Qa => LearningFocus::FileList,
            };
        }
    }

    /// Move the Q&A history cursor.
    pub fn learning_select_qa(&mut self, delta: isize) {
        if let AppMode::Learning(state) = &mut self.mode {
            let len = state.qa.len();
            if len == 0 {
                return;
            }
            state.selected_qa =
                (state.selected_qa as isize + delta).clamp(0, len as isize - 1) as usize;
        }
    }

    /// `Enter`: what it does depends on the focused pane — expand/collapse the
    /// orientation group or load a file, or open the selected answer.
    pub fn learning_activate_selection(&mut self) {
        let focus = match &self.mode {
            AppMode::Learning(state) => state.focus,
            _ => return,
        };
        match focus {
            LearningFocus::FileList => {
                let on_header = matches!(
                    &self.mode,
                    AppMode::Learning(state)
                        if matches!(
                            state.selected_entry(),
                            Some(LearningListEntry::StartHereHeader)
                        )
                );
                if on_header {
                    self.learning_toggle_start_here();
                } else {
                    self.learning_load_selected_content();
                    if let AppMode::Learning(state) = &mut self.mode {
                        state.focus = LearningFocus::Content;
                    }
                }
            }
            LearningFocus::Content => {}
            LearningFocus::Qa => self.learning_open_answer(),
        }
    }

    /// Show the selected answer full-width as rendered markdown.
    pub fn learning_open_answer(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            let has_answer = state
                .qa
                .get(state.selected_qa)
                .is_some_and(|r| r.answer.is_some() || r.error.is_some());
            if !has_answer {
                return;
            }
            state.answer_open = true;
            state.answer_scroll = 0;
            // Force a re-render: the cache is keyed on width, not content.
            state.answer_rendered_lines.clear();
            state.answer_rendered_width = 0;
        }
    }

    pub fn learning_close_answer(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.answer_open = false;
            state.answer_scroll = 0;
            state.answer_rendered_lines.clear();
            state.answer_rendered_width = 0;
        }
    }

    pub fn learning_answer_scroll(&mut self, delta: isize) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.answer_scroll = (state.answer_scroll as isize + delta).max(0) as usize;
        }
    }

    pub fn learning_answer_scroll_to_top(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.answer_scroll = 0;
        }
    }

    /// Jump past the end; `draw_markdown_document` clamps to the real bottom
    /// once it knows how tall the rendered document is.
    pub fn learning_answer_scroll_to_bottom(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.answer_scroll = state.answer_rendered_lines.len().max(usize::MAX / 2);
        }
    }
}

// ── starter questions ────────────────────────────────────────

/// Which anchors a starter question makes sense for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarterScope {
    /// Only with the whole project selected.
    Project,
    /// With a file open (any anchor inside it included).
    File,
    /// Only with a line range or hunk selected.
    Lines,
}

/// A preset question. The point is that a blank prompt is never the only
/// option: a user who doesn't yet know what to ask still has a first move.
#[derive(Debug, Clone, Copy)]
pub struct StarterQuestion {
    pub text: &'static str,
    pub intent: LearningQaIntent,
    pub scope: StarterScope,
}

/// The v1 preset list. Written for someone reading code they didn't write;
/// see the plan's "the starter-question list is a guess".
pub const STARTER_QUESTIONS: &[StarterQuestion] = &[
    StarterQuestion {
        text: "Give me a tour of this project — what is it, and where does execution start?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Project,
    },
    StarterQuestion {
        text: "What should I read first to understand this project, and in what order?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Project,
    },
    StarterQuestion {
        text: "What is this file responsible for, and what do I need to know to read it?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::File,
    },
    StarterQuestion {
        text: "What calls into this file, and what does it call?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::File,
    },
    StarterQuestion {
        text: "Explain this line by line.",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Lines,
    },
    StarterQuestion {
        text: "Why is it written this way instead of the obvious way?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Lines,
    },
    StarterQuestion {
        text: "What would break if I deleted this?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Lines,
    },
    StarterQuestion {
        text: "What do the unfamiliar words here mean?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Lines,
    },
    StarterQuestion {
        text: "Suggest how to make this clearer without changing behaviour.",
        intent: LearningQaIntent::Action,
        scope: StarterScope::Lines,
    },
];

/// Indices of the starter questions worth offering for `anchor`.
pub fn starter_questions_for(anchor: LearningAnchor) -> Vec<usize> {
    STARTER_QUESTIONS
        .iter()
        .enumerate()
        .filter(|(_, q)| match (q.scope, anchor) {
            (StarterScope::Project, LearningAnchor::Project) => true,
            // A file-level question still applies when a range inside that
            // file is selected — the file is open either way.
            (StarterScope::File, LearningAnchor::File)
            | (StarterScope::File, LearningAnchor::Lines { .. })
            | (StarterScope::File, LearningAnchor::Hunk { .. }) => true,
            (StarterScope::Lines, LearningAnchor::Lines { .. })
            | (StarterScope::Lines, LearningAnchor::Hunk { .. }) => true,
            _ => false,
        })
        .map(|(i, _)| i)
        .collect()
}

// ── the question prompt ──────────────────────────────────────

impl App {
    /// Open the question prompt for `intent`, capturing the anchor as it
    /// stands so browsing can't move it under the user.
    pub fn learning_open_question(
        &mut self,
        intent: LearningQaIntent,
        parent_qa_id: Option<String>,
    ) {
        let (text, is_diff) = match &self.mode {
            AppMode::Learning(state) => (selection_text(state), state.selection_is_diff()),
            _ => return,
        };
        if let AppMode::Learning(state) = &mut self.mode {
            state.question = Some(crate::app::LearningQuestionEditor {
                editor: crate::editor::TextEditor::new(String::new()),
                intent,
                parent_qa_id,
                anchor: state.anchor,
                file_path: match state.anchor {
                    LearningAnchor::Project => None,
                    _ => state.content_path.clone(),
                },
                selection_text: text,
                selection_is_diff: is_diff,
                scroll: 0,
                sync_to_cursor: true,
            });
        }
    }

    /// Ask a follow-up to the selected answer.
    ///
    /// A newcomer's second question ("wait, what's a trait?") matters as much
    /// as their first, so the prompt opens carrying the parent's place in the
    /// project *and* its question and answer — the agent answers against what
    /// the user was just told rather than re-deriving it.
    pub fn learning_open_follow_up(&mut self) {
        let Some(parent) = (match &self.mode {
            AppMode::Learning(state) => state.qa.get(state.selected_qa).cloned(),
            _ => return,
        }) else {
            if let AppMode::Learning(state) = &mut self.mode {
                state.error =
                    Some("Ask something first — a follow-up continues an earlier answer.".into());
            }
            return;
        };
        if parent.answer.is_none() {
            if let AppMode::Learning(state) = &mut self.mode {
                state.error = Some(match parent.status {
                    crate::app::LearningQaStatus::Failed => {
                        "That question never got an answer to follow up on. Ask it again first."
                            .to_string()
                    }
                    _ => "That answer is still generating — you can follow up once it arrives."
                        .to_string(),
                });
            }
            return;
        }
        if let AppMode::Learning(state) = &mut self.mode {
            state.error = None;
            state.answer_open = false;
            state.question = Some(crate::app::LearningQuestionEditor {
                editor: crate::editor::TextEditor::new(String::new()),
                // Inherited, not re-chosen: a follow-up to an explanation is
                // still an explanation unless the user flips it with Ctrl+E.
                intent: parent.intent,
                parent_qa_id: Some(parent.id.clone()),
                anchor: parent.anchor,
                file_path: parent.file_path.clone(),
                selection_text: parent.selection_text.clone(),
                // From the parent row, not the live overlay: the user may have
                // browsed out of branch-changes scope since it was answered.
                selection_is_diff: parent.selection_is_diff,
                scroll: 0,
                sync_to_cursor: true,
            });
        }
    }

    /// Flip explain ⇄ change without losing what's been typed.
    pub fn learning_question_toggle_intent(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode
            && let Some(q) = &mut state.question
        {
            q.intent = q.intent.toggled();
        }
    }

    pub fn learning_cancel_question(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.question = None;
            state.starter_picker = None;
        }
    }

    /// Ask what's in the prompt. Returns the new row's id.
    pub fn learning_submit_question(&mut self) -> Option<String> {
        let (text, intent, parent, captured) = match &self.mode {
            AppMode::Learning(state) => {
                let q = state.question.as_ref()?;
                // The prompt captured its place when it opened; honour that
                // capture rather than re-reading the cursor, so a follow-up
                // asks about its parent's code and not wherever browsing left
                // the file list.
                let captured = q.parent_qa_id.as_ref().map(|_| AskAnchor {
                    anchor: q.anchor,
                    file_path: q.file_path.clone(),
                    selection_text: q.selection_text.clone(),
                    selection_is_diff: q.selection_is_diff,
                });
                (
                    q.editor.text().to_string(),
                    q.intent,
                    q.parent_qa_id.clone(),
                    captured,
                )
            }
            _ => return None,
        };
        if text.trim().is_empty() {
            return None;
        }
        if let AppMode::Learning(state) = &mut self.mode {
            state.question = None;
            state.starter_picker = None;
        }
        self.learning_ask_at(&text, intent, parent, captured)
    }

    /// Offer the presets that fit the current anchor. Opens the prompt first
    /// if it isn't already open, so the picker is a way *into* asking.
    pub fn learning_open_starter_picker(&mut self) {
        let anchor = match &self.mode {
            AppMode::Learning(state) => state
                .question
                .as_ref()
                .map(|q| q.anchor)
                .unwrap_or(state.anchor),
            _ => return,
        };
        let indices = starter_questions_for(anchor);
        if indices.is_empty() {
            if let AppMode::Learning(state) = &mut self.mode {
                state.error = Some(
                    "No starter questions fit what's selected — pick a file or some lines first."
                        .to_string(),
                );
            }
            return;
        }
        if matches!(&self.mode, AppMode::Learning(state) if state.question.is_none()) {
            self.learning_open_question(LearningQaIntent::Explain, None);
        }
        if let AppMode::Learning(state) = &mut self.mode {
            state.starter_picker = Some(crate::app::LearningStarterPicker {
                indices,
                selected: 0,
            });
            state.error = None;
        }
    }

    pub fn learning_starter_picker_move(&mut self, delta: isize) {
        if let AppMode::Learning(state) = &mut self.mode
            && let Some(picker) = &mut state.starter_picker
        {
            let len = picker.indices.len();
            if len == 0 {
                return;
            }
            picker.selected = (picker.selected as isize + delta).rem_euclid(len as isize) as usize;
        }
    }

    pub fn learning_close_starter_picker(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.starter_picker = None;
        }
    }

    /// Load the highlighted preset into the prompt — editable, not asked.
    pub fn learning_starter_picker_confirm(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            let picked = state
                .starter_picker
                .as_ref()
                .and_then(|p| p.indices.get(p.selected).copied())
                .and_then(|i| STARTER_QUESTIONS.get(i));
            state.starter_picker = None;
            if let (Some(preset), Some(q)) = (picked, &mut state.question) {
                q.editor = crate::editor::TextEditor::new(preset.text.to_string());
                q.intent = preset.intent;
                q.sync_to_cursor = true;
            }
        }
    }
}

// ── help / first open ────────────────────────────────────────

impl App {
    pub fn learning_open_help(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.help_open = true;
            state.help_scroll = 0;
        }
    }

    pub fn learning_close_help(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.help_open = false;
            state.help_scroll = 0;
        }
    }

    pub fn learning_help_scroll(&mut self, delta: isize) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.help_scroll = (state.help_scroll as isize + delta).max(0) as usize;
        }
    }

    /// On the project's first visit, open the help overlay unprompted and
    /// remember that it's been shown. A newcomer's discovery path is this
    /// overlay, not the source.
    fn learning_show_onboarding_if_new(&mut self) {
        let (session_id, seen) = match &self.mode {
            AppMode::Learning(state) => (state.session_id.clone(), state.help_open),
            _ => return,
        };
        let _ = seen;
        if session_id.is_empty() {
            return;
        }
        let already_seen = self
            .db
            .as_ref()
            .and_then(|db| db.learning_session_onboarding_seen(&session_id).ok())
            .unwrap_or(true);
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

// ── session settings (level, harness) ────────────────────────

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

    pub fn learning_close_harness_picker(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.harness_picker = None;
        }
    }

    fn persist_learning_settings(
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

/// Turn a headless failure into something a newcomer can act on. The common
/// case by far is "that CLI isn't installed", which has a specific fix.
fn headless_failure_message(harness: &AgentKind, err: &anyhow::Error) -> String {
    let raw = err.to_string();
    let name = harness.display_name();
    if raw.contains("not found") || raw.contains("No such file") {
        format!(
            "{name} isn't installed or isn't on your PATH, so it couldn't answer. \
             Press A on the dashboard to set up a harness, or switch harness here."
        )
    } else {
        format!("{name} couldn't answer: {raw}. Try again, or switch harness here.")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::{ProjectStatus, ProjectStore};
    use crate::project::{Feature, Project, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::process::Command;
    use tempfile::TempDir;

    /// A store with one project/feature rooted at `workdir`.
    fn store_at(workdir: &Path, is_git: bool) -> ProjectStore {
        let now = chrono::Utc::now();
        let feature = Feature {
            id: "feat-1".to_string(),
            name: "my-feat".to_string(),
            branch: "my-feat".to_string(),
            workdir: workdir.to_path_buf(),
            is_worktree: false,
            tmux_session: "amf-my-feat".to_string(),
            sessions: vec![],
            collapsed: false,
            mode: VibeMode::default(),
            review: false,
            plan_mode: false,
            agent: AgentKind::default(),
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Stopped,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
            triage_source: None,
        };
        ProjectStore {
            version: 2,
            projects: vec![Project {
                id: "proj-1".to_string(),
                name: "my-project".to_string(),
                repo: workdir.to_path_buf(),
                collapsed: false,
                features: vec![feature],
                created_at: now,
                preferred_agent: AgentKind::default(),
                is_git,
            }],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: std::collections::HashMap::new(),
        }
    }

    fn app_at(workdir: &Path, is_git: bool) -> App {
        App::new_for_test(
            store_at(workdir, is_git),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        )
    }

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repo on `main` with a README, a source file, and a branch carrying one
    /// changed file.
    fn repo_with_branch_change() -> TempDir {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "--initial-branch=main"]);
        git(repo.path(), &["config", "user.name", "AMF Test"]);
        git(repo.path(), &["config", "user.email", "amf@example.com"]);
        std::fs::write(repo.path().join("README.md"), "# my-project\n").unwrap();
        std::fs::create_dir_all(repo.path().join("src")).unwrap();
        std::fs::write(repo.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(repo.path().join("src/util.rs"), "pub fn ok() {}\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "initial"]);
        git(repo.path(), &["checkout", "-b", "my-feat"]);
        std::fs::write(
            repo.path().join("src/main.rs"),
            "fn main() {\n    ok();\n}\n",
        )
        .unwrap();
        git(repo.path(), &["commit", "-am", "call ok"]);
        repo
    }

    fn learning(app: &App) -> &LearningViewState {
        match &app.mode {
            AppMode::Learning(state) => state,
            _ => panic!("expected Learning mode"),
        }
    }

    fn state_with_content(lines: &[&str]) -> LearningViewState {
        let mut state = LearningViewState::new(
            "proj-1".to_string(),
            0,
            0,
            "amf".to_string(),
            "learning-mode".to_string(),
            PathBuf::from("/tmp/does-not-matter"),
            true,
            AgentKind::Claude,
            LearningLevel::Newcomer,
            "sess-1".to_string(),
        );
        state.content = lines.iter().map(|l| (*l).to_string()).collect();
        state.content_path = Some("src/app/learning.rs".to_string());
        state
    }

    #[test]
    fn start_here_lists_only_files_that_exist() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("README.md"), "hi").unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

        let found = start_here_candidates(dir.path());
        assert_eq!(found, vec!["README.md", "src/main.rs", "Cargo.toml"]);
    }

    #[test]
    fn start_here_is_empty_for_a_project_following_no_conventions() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("thing.xyz"), "?").unwrap();
        assert!(start_here_candidates(dir.path()).is_empty());
    }

    /// A directory named like a candidate isn't a reading suggestion.
    #[test]
    fn start_here_ignores_directories() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("README.md")).unwrap();
        assert!(start_here_candidates(dir.path()).is_empty());
    }

    #[test]
    fn repo_tree_entries_pin_the_orientation_group_on_top() {
        let entries = build_repo_tree_entries(
            &["src/app/learning.rs".to_string(), "README.md".to_string()],
            &["README.md".to_string()],
            false,
        );
        assert!(matches!(entries[0], LearningListEntry::StartHereHeader));
        assert!(matches!(entries[1], LearningListEntry::ProjectTour));
        assert_eq!(entries[2].path(), Some("README.md"));
        // The pinned copy doesn't remove the file from the full list below.
        assert_eq!(entries.len(), 5);
    }

    #[test]
    fn collapsing_the_group_keeps_only_its_header() {
        let entries = build_repo_tree_entries(
            &["src/main.rs".to_string()],
            &["README.md".to_string()],
            true,
        );
        assert!(matches!(entries[0], LearningListEntry::StartHereHeader));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].path(), Some("src/main.rs"));
    }

    #[test]
    fn no_orientation_group_when_no_candidate_exists() {
        let entries = build_repo_tree_entries(&["a.rs".to_string()], &[], false);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_file());
    }

    #[test]
    fn line_anchor_is_one_based_and_clamped() {
        let mut state = state_with_content(&["a", "b", "c"]);
        state.cursor_line = 0;
        assert_eq!(
            anchor_for_cursor(&state),
            LearningAnchor::Lines { start: 1, end: 1 }
        );

        state.cursor_line = 2;
        state.selection_anchor = Some(0);
        assert_eq!(
            anchor_for_cursor(&state),
            LearningAnchor::Lines { start: 1, end: 3 }
        );

        // A cursor past the end (file reloaded shorter) clamps rather than
        // producing an anchor that points off the end of the file.
        state.cursor_line = 99;
        state.selection_anchor = None;
        assert_eq!(
            anchor_for_cursor(&state),
            LearningAnchor::Lines { start: 3, end: 3 }
        );
    }

    #[test]
    fn empty_file_anchors_to_the_file() {
        let state = state_with_content(&[]);
        assert_eq!(anchor_for_cursor(&state), LearningAnchor::File);
    }

    #[test]
    fn selection_text_covers_the_anchored_lines_only() {
        let mut state = state_with_content(&["one", "two", "three"]);
        state.cursor_line = 1;
        state.selection_anchor = Some(2);
        state.anchor = anchor_for_cursor(&state);
        assert_eq!(selection_text(&state), "two\nthree");

        state.anchor = LearningAnchor::File;
        assert_eq!(selection_text(&state), "one\ntwo\nthree");

        state.anchor = LearningAnchor::Project;
        assert_eq!(selection_text(&state), "");
    }

    /// Repo-tree browsing has no diff, so there is no hunk to select.
    #[test]
    fn hunk_selection_is_unavailable_in_repo_tree_scope() {
        let mut state = state_with_content(&["a"]);
        state.scope = BrowseScope::RepoTree;
        assert!(!state.hunk_selection_available());

        // Nor is it available in branch-changes scope with nothing selected.
        state.scope = BrowseScope::BranchChanges;
        assert!(!state.hunk_selection_available());
    }

    #[test]
    fn hunk_lookup_finds_the_enclosing_hunk() {
        let starts = vec![0usize, 5, 12];
        assert_eq!(hunk_index_for_line(&starts, 0), Some(0));
        assert_eq!(hunk_index_for_line(&starts, 4), Some(0));
        assert_eq!(hunk_index_for_line(&starts, 5), Some(1));
        assert_eq!(hunk_index_for_line(&starts, 11), Some(1));
        assert_eq!(hunk_index_for_line(&starts, 40), Some(2));
        assert_eq!(hunk_index_for_line(&[], 3), None);
    }

    #[test]
    fn binary_and_oversized_files_are_skipped_with_a_reason() {
        let dir = TempDir::new().unwrap();
        let binary = dir.path().join("thing.bin");
        std::fs::write(&binary, [0x7f, 0x45, 0x00, 0x01]).unwrap();
        let err = load_file_lines(&binary).unwrap_err();
        assert!(err.contains("binary"), "{err}");

        let big = dir.path().join("huge.txt");
        std::fs::write(&big, vec![b'a'; (MAX_FILE_BYTES + 1) as usize]).unwrap();
        let err = load_file_lines(&big).unwrap_err();
        assert!(err.contains("too big"), "{err}");

        let missing = dir.path().join("nope.txt");
        assert!(load_file_lines(&missing).is_err());
    }

    #[test]
    fn text_files_load_as_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        assert_eq!(load_file_lines(&path).unwrap(), vec!["one", "two"]);
    }

    // ── prompt builders ──────────────────────────────────────

    fn sample_context() -> LearningPromptContext {
        LearningPromptContext {
            project_name: "my-project".to_string(),
            feature_name: "learning-mode".to_string(),
            file_path: Some("src/app/learning.rs".to_string()),
            anchor: LearningAnchor::Lines { start: 3, end: 4 },
            selection_text: "let x = 1;\nlet y = 2;".to_string(),
            selection_is_diff: false,
            file_lines: (1..=10).map(|i| format!("line {i}")).collect(),
            selection_start_line: Some(3),
            question: "What does this do?".to_string(),
            intent: LearningQaIntent::Explain,
            level: LearningLevel::Newcomer,
            run_mode: crate::app::LearningRunMode::NoTools,
            ancestors: Vec::new(),
        }
    }

    #[test]
    fn every_prompt_carries_identity_path_numbered_selection_and_context() {
        let prompt = build_prompt(&sample_context());

        assert!(prompt.contains("Project: my-project"), "{prompt}");
        assert!(
            prompt.contains("Branch / feature: learning-mode"),
            "{prompt}"
        );
        assert!(prompt.contains("File: src/app/learning.rs"), "{prompt}");
        assert!(
            prompt.contains("lines 3-4 of src/app/learning.rs"),
            "{prompt}"
        );
        // The selection is numbered from its real line, not from 1.
        assert!(prompt.contains("     3 | let x = 1;"), "{prompt}");
        assert!(prompt.contains("     4 | let y = 2;"), "{prompt}");
        assert!(prompt.contains("Surrounding context"), "{prompt}");
        assert!(prompt.contains("What does this do?"), "{prompt}");
    }

    #[test]
    fn the_explain_template_never_asks_for_a_change() {
        let prompt = build_prompt(&sample_context());
        assert!(prompt.contains("Do not propose changes"), "{prompt}");
        assert!(
            !prompt.contains("smallest concrete change"),
            "explain must not carry the action instruction: {prompt}"
        );
        assert!(!prompt.contains("imperative summary"), "{prompt}");
    }

    #[test]
    fn the_action_template_asks_for_a_usable_one_line_title() {
        let mut ctx = sample_context();
        ctx.intent = LearningQaIntent::Action;
        let prompt = build_prompt(&ctx);
        assert!(prompt.contains("smallest concrete change"), "{prompt}");
        assert!(prompt.contains("imperative summary"), "{prompt}");
        assert!(prompt.contains("title of a work item"), "{prompt}");
        assert!(!prompt.contains("Do not propose changes"), "{prompt}");
    }

    /// A deep dive is labelled "read the repo" on the row and in the answer
    /// pane. Read-only tools only make that possible, so the prompt has to be
    /// what makes it true.
    #[test]
    fn the_deep_dive_template_requires_the_repository_to_be_read() {
        let mut ctx = sample_context();
        ctx.run_mode = crate::app::LearningRunMode::DeepDive;
        let prompt = build_prompt(&ctx);

        assert!(
            prompt.contains("read-only access to this repository"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Ground every claim in what you actually read"),
            "permission is not enough — the reading has to be required: {prompt}"
        );
        assert!(
            prompt.contains("name the files and symbols you checked"),
            "and be checkable from the answer itself: {prompt}"
        );
        assert!(
            !prompt.contains("no access to the rest of the repository"),
            "the no-tools disclaimer must not survive into a run that has access: {prompt}"
        );
    }

    #[test]
    fn the_no_tools_template_says_it_cannot_see_the_rest_of_the_repository() {
        let prompt = build_prompt(&sample_context());
        assert!(
            prompt.contains("no access to the rest of the repository"),
            "{prompt}"
        );
        assert!(prompt.contains("Do not invent file paths"), "{prompt}");
        assert!(
            !prompt.contains("read-only access to this repository"),
            "{prompt}"
        );
    }

    #[test]
    fn the_newcomer_overlay_is_present_by_default_and_absent_when_familiar() {
        let newcomer = build_prompt(&sample_context());
        assert!(
            newcomer.contains("Define every technical term"),
            "{newcomer}"
        );
        assert!(newcomer.contains("Where to look next"), "{newcomer}");
        assert!(newcomer.contains("No question is too basic"), "{newcomer}");

        let mut ctx = sample_context();
        ctx.level = LearningLevel::Familiar;
        let familiar = build_prompt(&ctx);
        assert!(
            !familiar.contains("Define every technical term"),
            "{familiar}"
        );
        assert!(
            !familiar.contains("Finish with a section headed"),
            "{familiar}"
        );
        assert!(
            familiar.contains("Be dense and skip the basics"),
            "{familiar}"
        );
    }

    #[test]
    fn a_follow_up_carries_its_parent_exactly_once() {
        let mut ctx = sample_context();
        ctx.question = "What's a trait?".to_string();
        ctx.ancestors = vec![ParentTurn {
            question: "What does this do?".to_string(),
            answer: "It implements a trait.".to_string(),
        }];
        let prompt = build_prompt(&ctx);

        assert!(prompt.contains("Earlier in this conversation"), "{prompt}");
        assert_eq!(
            prompt.matches("It implements a trait.").count(),
            1,
            "parent answer should appear once: {prompt}"
        );
        assert_eq!(
            prompt.matches("They asked: What does this do?").count(),
            1,
            "{prompt}"
        );
        assert!(prompt.contains("What's a trait?"), "{prompt}");
    }

    #[test]
    fn follow_up_context_is_capped_at_the_configured_depth() {
        let mut ctx = sample_context();
        ctx.ancestors = (1..=6)
            .map(|i| ParentTurn {
                question: format!("question {i}"),
                answer: format!("answer {i}"),
            })
            .collect();
        let prompt = build_prompt(&ctx);

        // The oldest ancestors are trimmed; the most recent survive.
        assert!(!prompt.contains("answer 1"), "{prompt}");
        assert!(!prompt.contains("answer 3"), "{prompt}");
        assert!(prompt.contains("answer 4"), "{prompt}");
        assert!(prompt.contains("answer 6"), "{prompt}");
        assert_eq!(prompt.matches("They asked:").count(), MAX_FOLLOW_UP_DEPTH);
    }

    #[test]
    fn the_project_anchor_prompt_has_no_file_or_selection() {
        let mut ctx = sample_context();
        ctx.anchor = LearningAnchor::Project;
        ctx.file_path = None;
        ctx.selection_text = String::new();
        ctx.question = "Give me a tour of this project.".to_string();
        let prompt = build_prompt(&ctx);

        assert!(prompt.contains("this whole project"), "{prompt}");
        assert!(!prompt.contains("File: "), "{prompt}");
        assert!(!prompt.contains("Surrounding context"), "{prompt}");
        assert!(prompt.contains("about the project as a whole"), "{prompt}");
    }

    /// A whole-file anchor already carries the file, so repeating it as
    /// "surrounding context" would double the prompt for no gain.
    #[test]
    fn a_whole_file_anchor_does_not_repeat_the_file_as_context() {
        let mut ctx = sample_context();
        ctx.anchor = LearningAnchor::File;
        assert!(!build_prompt(&ctx).contains("Surrounding context"));
    }

    #[test]
    fn oversized_selections_are_truncated_with_a_marker() {
        let mut ctx = sample_context();
        let long: Vec<String> = (1..=(MAX_SELECTION_LINES + 50))
            .map(|i| format!("line {i}"))
            .collect();
        ctx.selection_text = long.join("\n");
        let prompt = build_prompt(&ctx);
        assert!(prompt.contains("50 more lines not shown"), "{prompt}");
    }

    #[test]
    fn prompt_context_comes_from_the_live_anchor() {
        let repo = repo_with_branch_change();
        let mut app = app_at(repo.path(), true);
        app.open_learning_mode(0, 0).unwrap();
        while learning(&app).content_path.as_deref() != Some("src/main.rs") {
            app.learning_select_next_entry();
        }
        app.learning_cursor_move(0);

        let ctx = app
            .learning_prompt_context("What is this?", LearningQaIntent::Explain, Vec::new())
            .unwrap();
        assert_eq!(ctx.file_path.as_deref(), Some("src/main.rs"));
        assert_eq!(ctx.project_name, "my-project");
        assert_eq!(ctx.level, LearningLevel::Newcomer);
        assert!(!ctx.file_lines.is_empty());
    }

    // ── asking, level, harness ───────────────────────────────

    /// Deliver an answer the way a finished thread would, without running a
    /// real harness.
    fn deliver(app: &mut App, qa_id: &str, result: Result<String, String>) {
        app.learning_answer_tx
            .send(LearningAnswer {
                qa_id: qa_id.to_string(),
                result,
            })
            .unwrap();
        assert!(app.poll_learning_answers_bg());
    }

    // ── starter questions ────────────────────────────────────

    fn starters_for(anchor: LearningAnchor) -> Vec<&'static str> {
        starter_questions_for(anchor)
            .into_iter()
            .map(|i| STARTER_QUESTIONS[i].text)
            .collect()
    }

    #[test]
    fn project_presets_are_offered_only_for_the_project_anchor() {
        let project_only: Vec<&str> = STARTER_QUESTIONS
            .iter()
            .filter(|q| q.scope == StarterScope::Project)
            .map(|q| q.text)
            .collect();
        assert!(!project_only.is_empty(), "the table has project presets");

        let offered = starters_for(LearningAnchor::Project);
        for text in &project_only {
            assert!(
                offered.contains(text),
                "{text} missing at the project anchor"
            );
        }
        for anchor in [
            LearningAnchor::File,
            LearningAnchor::Lines { start: 1, end: 3 },
        ] {
            let offered = starters_for(anchor);
            for text in &project_only {
                assert!(
                    !offered.contains(text),
                    "{text} should not be offered at {anchor:?}"
                );
            }
        }
    }

    #[test]
    fn line_presets_need_a_line_or_hunk_range() {
        let line_only: Vec<&str> = STARTER_QUESTIONS
            .iter()
            .filter(|q| q.scope == StarterScope::Lines)
            .map(|q| q.text)
            .collect();
        assert!(!line_only.is_empty(), "the table has line presets");

        for anchor in [
            LearningAnchor::Lines { start: 4, end: 9 },
            LearningAnchor::Hunk { index: 0 },
        ] {
            let offered = starters_for(anchor);
            for text in &line_only {
                assert!(offered.contains(text), "{text} missing at {anchor:?}");
            }
        }
        for anchor in [LearningAnchor::Project, LearningAnchor::File] {
            let offered = starters_for(anchor);
            for text in &line_only {
                assert!(
                    !offered.contains(text),
                    "{text} should not be offered at {anchor:?}"
                );
            }
        }
    }

    /// A file-level question still applies once a range inside that file is
    /// selected — the file is open either way.
    #[test]
    fn file_presets_apply_to_ranges_inside_that_file() {
        let file_only: Vec<&str> = STARTER_QUESTIONS
            .iter()
            .filter(|q| q.scope == StarterScope::File)
            .map(|q| q.text)
            .collect();
        for anchor in [
            LearningAnchor::File,
            LearningAnchor::Lines { start: 2, end: 2 },
            LearningAnchor::Hunk { index: 1 },
        ] {
            let offered = starters_for(anchor);
            for text in &file_only {
                assert!(offered.contains(text), "{text} missing at {anchor:?}");
            }
        }
        assert!(
            starters_for(LearningAnchor::Project)
                .iter()
                .all(|t| !file_only.contains(t)),
            "file presets need a file"
        );
    }

    /// Shared with `crate::handlers::learning`'s tests: a dashboard-mode app on
    /// a real temp repo, for exercising the `K` entry key.
    pub(crate) fn dashboard_app_for_handlers() -> (TempDir, App) {
        let repo = repo_with_branch_change();
        let app = app_at(repo.path(), true);
        (repo, app)
    }

    /// As above, but the project has no features — nothing for Learning Mode
    /// to read.
    pub(crate) fn featureless_app_for_handlers() -> (TempDir, App) {
        let repo = repo_with_branch_change();
        let mut app = app_at(repo.path(), true);
        app.store.projects[0].features.clear();
        (repo, app)
    }

    /// Shared with `crate::handlers::learning`'s tests: an overlay opened on a
    /// real temp repo with a file loaded.
    pub(crate) fn opened_app_for_handlers() -> (TempDir, App) {
        opened_app()
    }

    fn opened_app() -> (TempDir, App) {
        let repo = repo_with_branch_change();
        let mut app = app_at(repo.path(), true);
        app.open_learning_mode(0, 0).unwrap();
        while learning(&app).content_path.as_deref() != Some("src/main.rs") {
            app.learning_select_next_entry();
        }
        (repo, app)
    }

    #[test]
    fn asking_enqueues_a_row_and_returns_control_immediately() {
        let (_repo, mut app) = opened_app();

        let id = app
            .learning_ask("What does this do?", LearningQaIntent::Explain, None)
            .unwrap();

        let state = learning(&app);
        assert_eq!(state.qa.len(), 1);
        let row = &state.qa[0];
        assert_eq!(row.id, id);
        assert_eq!(row.question, "What does this do?");
        assert_eq!(row.intent, LearningQaIntent::Explain);
        assert_eq!(row.level, LearningLevel::Newcomer);
        assert_eq!(row.file_path.as_deref(), Some("src/main.rs"));
        assert_eq!(row.run_mode, crate::app::LearningRunMode::NoTools);
        // Queued or already running — either way the user is not blocked.
        assert!(row.status.is_in_flight());
        assert_eq!(state.in_flight_count(), 1);
        // The overlay is still fully interactive.
        assert!(!state.entries.is_empty());
    }

    #[test]
    fn an_empty_question_is_not_enqueued() {
        let (_repo, mut app) = opened_app();
        assert!(
            app.learning_ask("   ", LearningQaIntent::Explain, None)
                .is_none()
        );
        assert!(learning(&app).qa.is_empty());
    }

    #[test]
    fn answers_land_on_their_own_row_and_clear_the_in_flight_count() {
        let (_repo, mut app) = opened_app();
        let first = app
            .learning_ask("Question one", LearningQaIntent::Explain, None)
            .unwrap();
        let second = app
            .learning_ask("Question two", LearningQaIntent::Action, None)
            .unwrap();
        assert_eq!(learning(&app).in_flight_count(), 2);

        // Out of order, as real runs finish.
        deliver(&mut app, &second, Ok("Second answer".to_string()));
        let state = learning(&app);
        assert_eq!(state.in_flight_count(), 1);
        assert_eq!(
            state.qa.iter().find(|r| r.id == second).unwrap().answer,
            Some("Second answer".to_string())
        );
        assert!(
            state
                .qa
                .iter()
                .find(|r| r.id == first)
                .unwrap()
                .answer
                .is_none()
        );

        deliver(&mut app, &first, Ok("First answer".to_string()));
        assert_eq!(learning(&app).in_flight_count(), 0);
    }

    #[test]
    fn a_failed_run_keeps_the_row_and_says_what_to_do() {
        let (_repo, mut app) = opened_app();
        let id = app
            .learning_ask("What does this do?", LearningQaIntent::Explain, None)
            .unwrap();

        deliver(
            &mut app,
            &id,
            Err("Claude isn't installed or isn't on your PATH".to_string()),
        );

        let row = &learning(&app).qa[0];
        assert_eq!(row.status, crate::app::LearningQaStatus::Failed);
        assert!(row.error.as_deref().unwrap().contains("isn't installed"));
        assert!(row.answer.is_none(), "the question survives for a retry");
    }

    #[test]
    fn a_missing_cli_failure_points_at_the_harness_wizard() {
        let msg =
            headless_failure_message(&AgentKind::Claude, &anyhow::anyhow!("claude CLI not found"));
        assert!(msg.contains("Press A"), "{msg}");
        assert!(msg.contains("Claude"), "{msg}");
    }

    #[test]
    fn a_follow_up_inherits_the_anchor_and_carries_the_parent_forward() {
        let (_repo, mut app) = opened_app();
        let parent = app
            .learning_ask("What does this do?", LearningQaIntent::Explain, None)
            .unwrap();
        deliver(&mut app, &parent, Ok("It calls ok().".to_string()));

        let child = app
            .learning_ask(
                "What's a function?",
                LearningQaIntent::Explain,
                Some(parent.clone()),
            )
            .unwrap();

        let state = learning(&app);
        let row = state.qa.iter().find(|r| r.id == child).unwrap();
        assert_eq!(row.parent_qa_id.as_deref(), Some(parent.as_str()));
        assert_eq!(row.file_path.as_deref(), Some("src/main.rs"));

        // The prompt the follow-up would have been built with carries the
        // parent turn.
        let ancestors = app.learning_ancestor_turns(Some(&parent));
        assert_eq!(ancestors.len(), 1);
        assert_eq!(ancestors[0].answer, "It calls ok().");
    }

    /// An unanswered parent has nothing to contribute, so it isn't carried.
    #[test]
    fn unanswered_ancestors_are_skipped() {
        let (_repo, mut app) = opened_app();
        let parent = app
            .learning_ask("Pending question", LearningQaIntent::Explain, None)
            .unwrap();
        assert!(app.learning_ancestor_turns(Some(&parent)).is_empty());
    }

    #[test]
    fn toggling_level_affects_later_questions_only() {
        let (_repo, mut app) = opened_app();
        let first = app
            .learning_ask("Question one", LearningQaIntent::Explain, None)
            .unwrap();
        deliver(&mut app, &first, Ok("An answer".to_string()));

        app.learning_toggle_level();
        assert_eq!(learning(&app).level, LearningLevel::Familiar);

        let second = app
            .learning_ask("Question two", LearningQaIntent::Explain, None)
            .unwrap();
        let state = learning(&app);
        // The earlier row keeps the level it was answered at, and its text is
        // untouched.
        let old = state.qa.iter().find(|r| r.id == first).unwrap();
        assert_eq!(old.level, LearningLevel::Newcomer);
        assert_eq!(old.answer.as_deref(), Some("An answer"));
        assert_eq!(
            state.qa.iter().find(|r| r.id == second).unwrap().level,
            LearningLevel::Familiar
        );

        app.learning_toggle_level();
        assert_eq!(learning(&app).level, LearningLevel::Newcomer);
    }

    #[test]
    fn the_harness_picker_is_optional_and_pre_selected() {
        let repo = repo_with_branch_change();
        let mut store = store_at(repo.path(), true);
        store.available_harnesses = vec![AgentKind::Codex, AgentKind::Claude];
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.open_learning_mode(0, 0).unwrap();

        // Pre-selected from the first available harness — no picker needed.
        assert_eq!(learning(&app).harness, AgentKind::Codex);
        assert!(learning(&app).harness_picker.is_none());

        app.learning_open_harness_picker();
        let picker = learning(&app).harness_picker.as_ref().unwrap();
        assert_eq!(picker.harnesses.len(), 2);
        assert_eq!(picker.selected, 0, "opens on the harness in use");

        app.learning_harness_picker_move(1);
        app.learning_harness_picker_confirm();
        let state = learning(&app);
        assert_eq!(state.harness, AgentKind::Claude);
        assert!(state.harness_picker.is_none());
    }

    #[test]
    fn cancelling_the_harness_picker_changes_nothing() {
        let (_repo, mut app) = opened_app();
        let before = learning(&app).harness.clone();
        app.learning_open_harness_picker();
        app.learning_harness_picker_move(1);
        app.learning_close_harness_picker();
        assert_eq!(learning(&app).harness, before);
        assert!(learning(&app).harness_picker.is_none());
    }

    #[test]
    fn questions_record_the_harness_that_answers_them() {
        let (_repo, mut app) = opened_app();
        let id = app
            .learning_ask("What does this do?", LearningQaIntent::Explain, None)
            .unwrap();
        let expected = learning(&app).harness.clone();
        assert_eq!(
            learning(&app)
                .qa
                .iter()
                .find(|r| r.id == id)
                .unwrap()
                .harness,
            expected
        );
    }

    // ── overlay-level behaviour ──────────────────────────────

    #[test]
    fn opening_lists_repo_files_with_the_orientation_group_on_top() {
        let repo = repo_with_branch_change();
        let mut app = app_at(repo.path(), true);
        app.open_learning_mode(0, 0).unwrap();

        let state = learning(&app);
        assert_eq!(state.scope, BrowseScope::RepoTree);
        assert!(matches!(
            state.entries[0],
            LearningListEntry::StartHereHeader
        ));
        assert!(matches!(state.entries[1], LearningListEntry::ProjectTour));
        // The cursor opens on the tour question, not the group header.
        assert_eq!(state.selected_entry, 1);
        assert_eq!(state.anchor, LearningAnchor::Project);

        let paths: Vec<&str> = state.entries.iter().filter_map(|e| e.path()).collect();
        assert!(paths.contains(&"src/main.rs"), "{paths:?}");
        assert!(paths.contains(&"src/util.rs"), "{paths:?}");
        assert!(paths.contains(&"README.md"), "{paths:?}");
    }

    #[test]
    fn selecting_a_file_loads_its_content_and_a_line_anchor() {
        let repo = repo_with_branch_change();
        let mut app = app_at(repo.path(), true);
        app.open_learning_mode(0, 0).unwrap();

        // Walk to the first real file row.
        while learning(&app).content_path.is_none() {
            app.learning_select_next_entry();
        }
        let state = learning(&app);
        assert!(!state.content.is_empty());
        assert_eq!(state.anchor, LearningAnchor::File);

        app.learning_cursor_move(1);
        assert!(matches!(
            learning(&app).anchor,
            LearningAnchor::Lines { .. }
        ));
    }

    #[test]
    fn toggling_scope_switches_to_the_branch_s_changed_files_and_back() {
        let repo = repo_with_branch_change();
        let mut app = app_at(repo.path(), true);
        app.open_learning_mode(0, 0).unwrap();

        app.learning_toggle_scope();
        let state = learning(&app);
        assert_eq!(state.scope, BrowseScope::BranchChanges);
        assert!(state.error.is_none(), "{:?}", state.error);
        let paths: Vec<&str> = state.entries.iter().filter_map(|e| e.path()).collect();
        assert_eq!(paths, vec!["src/main.rs"], "only the changed file");
        // No orientation group in this scope.
        assert!(
            !state
                .entries
                .iter()
                .any(|e| matches!(e, LearningListEntry::StartHereHeader))
        );

        app.learning_toggle_scope();
        assert_eq!(learning(&app).scope, BrowseScope::RepoTree);
    }

    #[test]
    fn hunk_selection_works_only_once_a_changed_file_is_loaded() {
        let repo = repo_with_branch_change();
        let mut app = app_at(repo.path(), true);
        app.open_learning_mode(0, 0).unwrap();

        // Repo-tree scope: the key explains itself rather than doing nothing.
        app.learning_select_hunk();
        let err = learning(&app).error.clone().unwrap();
        assert!(err.contains("branch changes"), "{err}");

        app.learning_toggle_scope();
        assert!(learning(&app).hunk_selection_available());
        app.learning_select_hunk();
        let state = learning(&app);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert!(matches!(state.anchor, LearningAnchor::Hunk { index: 0 }));
        assert!(!selection_text(state).is_empty());
    }

    /// Branch-changes scope needs git, so a plain directory says so instead of
    /// showing an empty list.
    #[test]
    fn a_non_git_project_stays_in_repo_tree_scope_and_explains_why() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("main.py"), "print('hi')\n").unwrap();
        let mut app = app_at(dir.path(), false);
        app.open_learning_mode(0, 0).unwrap();

        let paths: Vec<&str> = learning(&app)
            .entries
            .iter()
            .filter_map(|e| e.path())
            .collect();
        assert_eq!(paths, vec!["main.py"], "falls back to a plain walk");

        app.learning_toggle_scope();
        let state = learning(&app);
        assert_eq!(state.scope, BrowseScope::RepoTree);
        let err = state.error.clone().unwrap();
        assert!(err.contains("git repository"), "{err}");
    }

    /// With no DB (as in tests) the overlay still opens and browses; history is
    /// simply empty and nothing is persisted.
    #[test]
    fn the_overlay_works_without_a_database() {
        let repo = repo_with_branch_change();
        let mut app = app_at(repo.path(), true);
        assert!(app.db.is_none());
        app.open_learning_mode(0, 0).unwrap();
        assert!(learning(&app).qa.is_empty());
        assert!(learning(&app).session_id.is_empty());
        assert!(!learning(&app).entries.is_empty());
    }

    #[test]
    fn closing_returns_to_the_feature_it_was_opened_from() {
        let repo = repo_with_branch_change();
        let mut app = app_at(repo.path(), true);
        app.open_learning_mode(0, 0).unwrap();
        app.close_learning_mode();
        assert!(matches!(app.mode, AppMode::Normal));
        assert!(matches!(app.selection, Selection::Feature(0, 0)));
    }

    #[test]
    fn collapsing_the_orientation_group_keeps_the_cursor_on_its_file() {
        let repo = repo_with_branch_change();
        let mut app = app_at(repo.path(), true);
        app.open_learning_mode(0, 0).unwrap();

        while learning(&app).selected_entry().and_then(|e| e.path()) != Some("src/util.rs") {
            app.learning_select_next_entry();
        }
        app.learning_toggle_start_here();

        let state = learning(&app);
        assert!(state.start_here_collapsed);
        assert_eq!(
            state.selected_entry().and_then(|e| e.path()),
            Some("src/util.rs")
        );
    }

    #[test]
    fn fallback_walk_skips_noise_and_respects_caps() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("main.py"), "print()").unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        std::fs::write(dir.path().join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join("lib/deep")).unwrap();
        std::fs::write(dir.path().join("lib/deep/util.py"), "y").unwrap();

        let files = walk_files_capped(dir.path(), 100, 12);
        assert_eq!(files, vec!["lib/deep/util.py", "main.py"]);

        // The entry cap truncates rather than growing without bound.
        assert_eq!(walk_files_capped(dir.path(), 1, 12).len(), 1);
        // The depth cap keeps the walk shallow.
        assert_eq!(walk_files_capped(dir.path(), 100, 0), vec!["main.py"]);
    }

    // ── follow-ups ───────────────────────────────────────────

    /// Ask, answer, and follow up — the loop a newcomer's second question
    /// depends on.
    fn ask_and_answer(app: &mut App, question: &str, answer: &str) -> String {
        let id = app
            .learning_ask(question, LearningQaIntent::Explain, None)
            .unwrap();
        deliver(app, &id, Ok(answer.to_string()));
        id
    }

    fn follow_up(app: &mut App, question: &str) -> String {
        app.learning_open_follow_up();
        assert!(
            learning(app).question.is_some(),
            "the follow-up prompt should be open"
        );
        for c in question.chars() {
            if let AppMode::Learning(state) = &mut app.mode
                && let Some(q) = &mut state.question
            {
                q.editor.insert_str(&c.to_string());
            }
        }
        app.learning_submit_question().unwrap()
    }

    #[test]
    fn a_follow_up_carries_its_parents_question_and_answer_into_the_prompt() {
        let (_repo, mut app) = opened_app();
        let parent = ask_and_answer(&mut app, "What is this file for?", "It is the entry point.");

        let child = follow_up(&mut app, "What's an entry point?");

        let state = learning(&app);
        let row = state.qa.iter().find(|r| r.id == child).unwrap();
        assert_eq!(row.parent_qa_id.as_deref(), Some(parent.as_str()));
        assert_eq!(
            row.intent,
            LearningQaIntent::Explain,
            "a follow-up inherits its parent's intent"
        );

        // The prompt the agent would receive carries the earlier turn verbatim.
        let ancestors = app.learning_ancestor_turns(Some(&parent));
        let ctx = app
            .learning_prompt_context(
                "What's an entry point?",
                LearningQaIntent::Explain,
                ancestors,
            )
            .unwrap();
        let prompt = build_prompt(&ctx);
        assert!(prompt.contains("What is this file for?"), "{prompt}");
        assert!(prompt.contains("It is the entry point."), "{prompt}");
        assert_eq!(
            prompt.matches("It is the entry point.").count(),
            1,
            "the parent answer appears exactly once"
        );
    }

    #[test]
    fn a_two_deep_follow_up_keeps_the_whole_conversation() {
        let (_repo, mut app) = opened_app();
        ask_and_answer(&mut app, "What is this file for?", "It is the entry point.");

        let second = follow_up(&mut app, "What's an entry point?");
        deliver(&mut app, &second, Ok("Where execution starts.".to_string()));
        let third = follow_up(&mut app, "What is execution?");

        let ancestors = app.learning_ancestor_turns(Some(&second));
        assert_eq!(ancestors.len(), 2, "both earlier turns come through");
        // Oldest first, so the agent reads the conversation in order.
        assert_eq!(ancestors[0].question, "What is this file for?");
        assert_eq!(ancestors[1].question, "What's an entry point?");

        let state = learning(&app);
        let row = state.qa.iter().find(|r| r.id == third).unwrap();
        assert_eq!(row.parent_qa_id.as_deref(), Some(second.as_str()));
    }

    #[test]
    fn a_follow_up_asks_about_its_parents_code_not_wherever_browsing_ended_up() {
        let (_repo, mut app) = opened_app();
        app.learning_cursor_move(1);
        let parent = ask_and_answer(&mut app, "Explain this line.", "It prints.");
        let (parent_anchor, parent_path, parent_text) = {
            let row = learning(&app).qa.iter().find(|r| r.id == parent).unwrap();
            (
                row.anchor,
                row.file_path.clone(),
                row.selection_text.clone(),
            )
        };

        // Browse somewhere else entirely before following up.
        app.learning_select_next_entry();
        app.learning_select_project();
        assert_ne!(learning(&app).anchor, parent_anchor);

        let child = follow_up(&mut app, "What does printing mean here?");

        let state = learning(&app);
        let row = state.qa.iter().find(|r| r.id == child).unwrap();
        assert_eq!(row.anchor, parent_anchor, "same place as its parent");
        assert_eq!(row.file_path, parent_path);
        assert_eq!(row.selection_text, parent_text);
    }

    #[test]
    fn a_follow_up_lands_under_the_thread_it_continues() {
        let (_repo, mut app) = opened_app();
        let first = ask_and_answer(&mut app, "First question?", "First answer.");
        // A second, unrelated question that would otherwise sit between the
        // parent and its follow-up.
        let unrelated = app
            .learning_ask("Unrelated question?", LearningQaIntent::Explain, None)
            .unwrap();

        // Follow up on the *first* row, not the newest.
        if let AppMode::Learning(state) = &mut app.mode {
            state.selected_qa = 0;
        }
        let child = follow_up(&mut app, "Follow-up?");

        let ids: Vec<&str> = learning(&app).qa.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![first.as_str(), child.as_str(), unrelated.as_str()],
            "the follow-up sits directly under its parent"
        );
        assert_eq!(
            learning(&app).selected_qa,
            1,
            "and the cursor follows the new question"
        );
    }

    // ── deep dive ────────────────────────────────────────────

    #[test]
    fn a_deep_dive_re_asks_the_same_question_with_the_repo_readable() {
        let (_repo, mut app) = opened_app();
        let origin = ask_and_answer(&mut app, "What does this do?", "It runs the thing.");

        let deeper = app.learning_deep_dive().unwrap();

        let state = learning(&app);
        assert_eq!(state.qa.len(), 2, "the first answer survives its rerun");
        let first = &state.qa[0];
        assert_eq!(first.id, origin);
        assert_eq!(
            first.answer.as_deref(),
            Some("It runs the thing."),
            "the shallow answer is left alone so the two can be compared"
        );

        let row = &state.qa[1];
        assert_eq!(row.id, deeper);
        assert_eq!(row.run_mode, crate::app::LearningRunMode::DeepDive);
        assert_eq!(row.question, first.question, "the same question, re-asked");
        assert_eq!(row.intent, first.intent);
        assert_eq!(row.anchor, first.anchor);
        assert_eq!(row.selection_text, first.selection_text);
        assert_eq!(
            row.parent_qa_id.as_deref(),
            Some(origin.as_str()),
            "it renders indented under the answer it is checking"
        );
        assert_eq!(state.selected_qa, 1, "and the cursor follows it");
    }

    /// The point of a deep dive is an independent re-derivation. Handing the
    /// agent the answer it is checking would just get that answer back.
    #[test]
    fn a_deep_dive_does_not_feed_the_shallow_answer_back_to_the_agent() {
        let (_repo, mut app) = opened_app();
        ask_and_answer(
            &mut app,
            "What does this do?",
            "It calls into the widget pump.",
        );

        let origin = learning(&app).qa[0].clone();
        let prompt = build_prompt(&app.learning_deep_dive_context(&origin).unwrap());

        assert!(prompt.contains("What does this do?"), "{prompt}");
        assert!(
            !prompt.contains("widget pump"),
            "the answer under review must not be in the prompt reviewing it: {prompt}"
        );
        assert!(
            prompt.contains("Ground every claim in what you actually read"),
            "and the rerun is told to go and check, not merely allowed to: {prompt}"
        );
    }

    /// A deep dive of a follow-up still needs the turns that led to it — what
    /// it drops is only the one answer it is re-deriving.
    #[test]
    fn a_deep_dive_of_a_follow_up_keeps_the_conversation_above_it() {
        let (_repo, mut app) = opened_app();
        ask_and_answer(&mut app, "What is this file for?", "It is the entry point.");
        let child = follow_up(&mut app, "What's an entry point?");
        deliver(&mut app, &child, Ok("Where execution begins.".to_string()));

        let origin = learning(&app)
            .qa
            .iter()
            .find(|r| r.id == child)
            .unwrap()
            .clone();
        let prompt = build_prompt(&app.learning_deep_dive_context(&origin).unwrap());

        assert!(
            prompt.contains("It is the entry point."),
            "the parent turn survives: {prompt}"
        );
        assert!(
            !prompt.contains("Where execution begins."),
            "but not the answer being re-derived: {prompt}"
        );
    }

    #[test]
    fn a_deep_dive_keeps_the_level_its_original_was_answered_at() {
        let (_repo, mut app) = opened_app();
        let origin = ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
        assert_eq!(learning(&app).level, LearningLevel::Newcomer);

        // The user moves on to denser answers, then sends the old one deeper.
        app.learning_toggle_level();
        assert_eq!(learning(&app).level, LearningLevel::Familiar);
        let deeper = app.learning_deep_dive().unwrap();

        let state = learning(&app);
        let row = state.qa.iter().find(|r| r.id == deeper).unwrap();
        assert_eq!(
            row.level,
            LearningLevel::Newcomer,
            "a rerun reads like the answer it reruns, not like the current setting"
        );
        assert_eq!(
            state.qa.iter().find(|r| r.id == origin).unwrap().level,
            row.level
        );
    }

    #[test]
    fn a_deep_dive_asks_about_its_originals_code_not_wherever_browsing_ended_up() {
        let (_repo, mut app) = opened_app();
        app.learning_select_whole_file();
        ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
        let asked_about = learning(&app).qa[0].file_path.clone();
        assert!(asked_about.is_some());

        // Browse away before sending it deeper.
        app.learning_select_next_entry();
        app.learning_load_selected_content();
        app.learning_deep_dive().unwrap();

        let state = learning(&app);
        assert_eq!(
            state.qa[1].file_path, asked_about,
            "the rerun follows the question, not the cursor"
        );
        assert_eq!(state.qa[1].selection_text, state.qa[0].selection_text);
    }

    #[test]
    fn a_deep_dive_of_an_unanswered_question_says_to_wait() {
        let (_repo, mut app) = opened_app();
        app.learning_ask("What does this do?", LearningQaIntent::Explain, None)
            .unwrap();

        assert!(app.learning_deep_dive().is_none());

        let state = learning(&app);
        assert_eq!(state.qa.len(), 1, "nothing was started");
        assert!(
            state
                .error
                .as_deref()
                .is_some_and(|e| e.contains("still generating")),
            "got {:?}",
            state.error
        );
    }

    /// Which is also the Codex case: `effective_for` records those rows as deep
    /// dives up front, because `codex exec` has no no-tools mode.
    #[test]
    fn a_deep_dive_of_a_deep_dive_says_it_already_read_the_repo() {
        let (_repo, mut app) = opened_app();
        let origin = ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
        let deeper = app.learning_deep_dive().unwrap();
        deliver(
            &mut app,
            &deeper,
            Ok("It really runs the thing.".to_string()),
        );

        assert!(app.learning_deep_dive().is_none());

        let state = learning(&app);
        assert_eq!(state.qa.len(), 2, "no third row");
        assert!(
            state
                .error
                .as_deref()
                .is_some_and(|e| e.contains("already read the repository")),
            "got {:?}",
            state.error
        );
        assert_eq!(state.qa[0].id, origin);
    }

    #[test]
    fn a_second_deep_dive_jumps_to_the_one_you_already_have() {
        let (_repo, mut app) = opened_app();
        ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
        let deeper = app.learning_deep_dive().unwrap();
        deliver(
            &mut app,
            &deeper,
            Ok("It really runs the thing.".to_string()),
        );

        // Back to the original, and ask for a deep dive again.
        if let AppMode::Learning(state) = &mut app.mode {
            state.selected_qa = 0;
        }
        assert!(app.learning_deep_dive().is_none());

        let state = learning(&app);
        assert_eq!(state.qa.len(), 2, "the same run isn't paid for twice");
        assert_eq!(
            state.qa[state.selected_qa].id, deeper,
            "the cursor lands on the answer that already exists"
        );
    }

    /// A deep dive that failed is worth retrying — that is exactly when the
    /// user wants it — so a failed row must not be mistaken for one that
    /// already answered.
    #[test]
    fn a_failed_deep_dive_can_be_retried() {
        let (_repo, mut app) = opened_app();
        ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
        let first = app.learning_deep_dive().unwrap();
        deliver(
            &mut app,
            &first,
            Err("codex: command not found".to_string()),
        );

        if let AppMode::Learning(state) = &mut app.mode {
            state.selected_qa = 0;
        }
        let retry = app.learning_deep_dive().unwrap();

        assert_ne!(retry, first);
        assert_eq!(learning(&app).qa.len(), 3);
    }

    /// The whole point of a deep dive is to replace an answer that may have
    /// invented its evidence. If a follow-up on the verified answer walked the
    /// thread back into the shallow one, the fabrication would be handed to the
    /// agent as established fact one question later.
    #[test]
    fn a_follow_up_on_a_deep_dive_leaves_the_answer_it_replaced_behind() {
        let (_repo, mut app) = opened_app();
        ask_and_answer(
            &mut app,
            "What does this do?",
            "It calls into the widget pump.",
        );
        let deeper = app.learning_deep_dive().unwrap();
        deliver(
            &mut app,
            &deeper,
            Ok("It calls into the event loop.".to_string()),
        );

        let turns = app.learning_ancestor_turns(Some(&deeper));

        let answers: Vec<&str> = turns.iter().map(|t| t.answer.as_str()).collect();
        assert_eq!(
            answers,
            vec!["It calls into the event loop."],
            "only the verified answer continues the conversation"
        );
    }

    /// A deep dive of a follow-up steps over the turn it re-ran, not over the
    /// conversation that led there.
    #[test]
    fn a_follow_up_on_a_deep_dive_still_carries_the_turns_above_it() {
        let (_repo, mut app) = opened_app();
        ask_and_answer(&mut app, "What is this file for?", "It is the entry point.");
        let child = follow_up(&mut app, "What's an entry point?");
        deliver(&mut app, &child, Ok("Where execution begins.".to_string()));

        // Send the follow-up deeper, then continue from the deep dive.
        if let AppMode::Learning(state) = &mut app.mode {
            state.selected_qa = state.qa.iter().position(|r| r.id == child).unwrap();
        }
        let deeper = app.learning_deep_dive().unwrap();
        deliver(
            &mut app,
            &deeper,
            Ok("Where the process starts running.".to_string()),
        );

        let answers: Vec<String> = app
            .learning_ancestor_turns(Some(&deeper))
            .into_iter()
            .map(|t| t.answer)
            .collect();
        assert_eq!(
            answers,
            vec![
                "It is the entry point.".to_string(),
                "Where the process starts running.".to_string(),
            ],
            "the grandparent turn survives; only the re-derived one is dropped"
        );
    }

    /// Under Codex every row is recorded as a deep dive (`effective_for`), so
    /// "is this a rerun?" cannot be read off `run_mode` — doing so would strip
    /// an ordinary Codex follow-up of the answer it is following up on.
    #[test]
    fn a_codex_follow_up_is_not_mistaken_for_a_rerun() {
        let (_repo, mut app) = opened_app();
        if let AppMode::Learning(state) = &mut app.mode {
            state.harness = AgentKind::Codex;
        }
        let parent = ask_and_answer(&mut app, "What is this file for?", "It is the entry point.");
        assert_eq!(
            learning(&app).qa[0].run_mode,
            crate::app::LearningRunMode::DeepDive,
            "codex has no no-tools mode"
        );

        let child = follow_up(&mut app, "What's an entry point?");
        let row = learning(&app)
            .qa
            .iter()
            .find(|r| r.id == child)
            .unwrap()
            .clone();
        assert_eq!(row.parent_qa_id.as_deref(), Some(parent.as_str()));
        assert!(
            row.deep_dive_of.is_none(),
            "a follow-up replaces nothing, whatever mode it runs in"
        );
        assert_eq!(
            app.learning_ancestor_turns(Some(&parent))
                .into_iter()
                .map(|t| t.answer)
                .collect::<Vec<_>>(),
            vec!["It is the entry point.".to_string()],
        );
    }

    /// `D` on a row that reads the repository is refused whether or not it has
    /// landed, so the in-flight message must not promise it will work later.
    #[test]
    fn d_on_a_running_deep_dive_says_to_follow_up_not_to_wait() {
        let (_repo, mut app) = opened_app();
        ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
        let deeper = app.learning_deep_dive().unwrap();
        assert!(
            learning(&app)
                .qa
                .iter()
                .find(|r| r.id == deeper)
                .unwrap()
                .status
                .is_in_flight()
        );

        assert!(app.learning_deep_dive().is_none(), "the cursor is on it");

        let error = learning(&app).error.clone().unwrap_or_default();
        assert!(error.contains("already reading the repository"), "{error}");
        assert!(
            error.contains("(F)"),
            "and points at what does work: {error}"
        );
        assert_eq!(learning(&app).qa.len(), 2, "nothing was started");
    }

    /// The second `D` jumps to the run that exists — which, while it is still
    /// running, has not come back with anything to read.
    #[test]
    fn a_second_deep_dive_while_the_first_runs_says_it_is_still_going() {
        let (_repo, mut app) = opened_app();
        ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
        let deeper = app.learning_deep_dive().unwrap();

        if let AppMode::Learning(state) = &mut app.mode {
            state.selected_qa = 0;
        }
        assert!(app.learning_deep_dive().is_none());

        let state = learning(&app);
        assert_eq!(state.qa.len(), 2, "the same run isn't paid for twice");
        assert_eq!(state.qa[state.selected_qa].id, deeper);
        let error = state.error.clone().unwrap_or_default();
        assert!(
            error.contains("still reading the repository"),
            "an unfinished run must not be described as one that came back: {error}"
        );
    }

    /// A bare row, for the ordering helpers that only look at ids and parents.
    fn qa_row(id: &str, parent: Option<&str>) -> LearningQa {
        LearningQa {
            id: id.to_string(),
            session_id: "s".to_string(),
            parent_qa_id: parent.map(str::to_string),
            deep_dive_of: None,
            file_path: None,
            anchor: LearningAnchor::Project,
            selection_text: String::new(),
            selection_is_diff: false,
            question: id.to_string(),
            intent: LearningQaIntent::Explain,
            level: LearningLevel::Newcomer,
            answer: None,
            harness: AgentKind::Claude,
            run_mode: crate::app::LearningRunMode::NoTools,
            status: crate::app::LearningQaStatus::Pending,
            error: None,
            todo_id: None,
            spawned_session_id: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn a_thread_insert_lands_past_every_descendant() {
        let rows = vec![
            qa_row("a", None),
            qa_row("b", Some("a")),
            qa_row("c", Some("b")),
            qa_row("d", None),
        ];
        // Past the whole a → b → c thread, not just past `a`.
        assert_eq!(thread_insert_index(&rows, "a"), Some(3));
        assert_eq!(thread_insert_index(&rows, "b"), Some(3));
        assert_eq!(thread_insert_index(&rows, "c"), Some(3));
        assert_eq!(thread_insert_index(&rows, "d"), Some(4));
        assert_eq!(
            thread_insert_index(&rows, "gone"),
            None,
            "a stale parent appends rather than vanishing"
        );
    }

    #[test]
    fn following_up_on_an_unanswered_question_says_to_wait() {
        let (_repo, mut app) = opened_app();
        app.learning_ask("Still thinking?", LearningQaIntent::Explain, None)
            .unwrap();

        app.learning_open_follow_up();
        let state = learning(&app);
        assert!(state.question.is_none(), "nothing to follow up on yet");
        let error = state.error.as_deref().unwrap_or_default();
        assert!(error.contains("still generating"), "{error}");
    }

    #[test]
    fn a_huge_repo_listing_is_capped_and_says_so() {
        let mut small: Vec<String> = (0..5).map(|i| format!("f{i}.rs")).collect();
        assert_eq!(
            cap_repo_entries(&mut small, 10),
            None,
            "a list under the cap is left alone and reports nothing"
        );
        assert_eq!(small.len(), 5);

        let mut big: Vec<String> = (0..50).map(|i| format!("f{i}.rs")).collect();
        assert_eq!(
            cap_repo_entries(&mut big, 10),
            Some(50),
            "the original total comes back so the user can be told"
        );
        assert_eq!(big.len(), 10);
    }

    #[test]
    fn a_codex_question_is_recorded_as_the_deep_dive_it_will_actually_be() {
        use crate::app::LearningRunMode;

        // Codex has no no-tools headless mode, so a row claiming "this file
        // only" would misdescribe the command that ran.
        assert_eq!(
            LearningRunMode::NoTools.effective_for(&AgentKind::Codex),
            LearningRunMode::DeepDive
        );
        assert_eq!(
            LearningRunMode::DeepDive.effective_for(&AgentKind::Codex),
            LearningRunMode::DeepDive
        );
        for harness in [AgentKind::Claude, AgentKind::Opencode, AgentKind::Pi] {
            assert_eq!(
                LearningRunMode::NoTools.effective_for(&harness),
                LearningRunMode::NoTools,
                "{harness:?} answers without tools when asked to"
            );
        }
    }

    #[test]
    fn a_question_stranded_by_a_previous_run_reloads_as_failed_not_thinking() {
        let (_repo, mut app) = opened_app();
        app.learning_ask("What does this do?", LearningQaIntent::Explain, None)
            .unwrap();
        let stranded = learning(&app).qa[0].clone();
        assert!(stranded.status.is_in_flight());

        // A fresh process knows about no live runs, so the row is stranded.
        app.learning_runs_in_flight.clear();
        let rows = app.reconcile_interrupted_qa(vec![stranded.clone()]);
        assert_eq!(rows[0].status, crate::app::LearningQaStatus::Failed);
        assert_eq!(
            rows[0].question, stranded.question,
            "the question survives so it can be asked again"
        );
        let reason = rows[0].error.as_deref().unwrap_or_default();
        assert!(reason.contains("Ask it again"), "{reason}");

        // A run this process is genuinely still waiting on is left alone.
        app.learning_runs_in_flight.insert(stranded.id.clone());
        let rows = app.reconcile_interrupted_qa(vec![stranded.clone()]);
        assert_eq!(rows[0].status, stranded.status);
        assert!(rows[0].error.is_none());
    }

    // ── answers that outlive their overlay ───────────────────

    /// An overlay backed by a real database, so history survives a close.
    fn opened_app_with_db() -> (TempDir, TempDir, App) {
        let repo = repo_with_branch_change();
        let db_dir = TempDir::new().unwrap();
        let mut app = app_at(repo.path(), true);
        app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
        app.open_learning_mode(0, 0).unwrap();
        while learning(&app).content_path.as_deref() != Some("src/main.rs") {
            app.learning_select_next_entry();
        }
        (repo, db_dir, app)
    }

    /// A run outlives the overlay that started it: closing the overlay while a
    /// question is generating must not leave the stored row at "running", which
    /// would reload as a question that never finishes.
    #[test]
    fn an_answer_arriving_after_the_overlay_closed_is_still_saved() {
        let (_repo, _db, mut app) = opened_app_with_db();
        let id = app
            .learning_ask("What does this do?", LearningQaIntent::Explain, None)
            .unwrap();
        assert!(!learning(&app).session_id.is_empty(), "persisted session");

        app.close_learning_mode();
        assert!(matches!(app.mode, AppMode::Normal));
        deliver(&mut app, &id, Ok("It is the entry point.".to_string()));

        app.open_learning_mode(0, 0).unwrap();
        let row = learning(&app)
            .qa
            .iter()
            .find(|r| r.id == id)
            .expect("the question is still in history")
            .clone();
        assert_eq!(row.status, crate::app::LearningQaStatus::Answered);
        assert_eq!(row.answer.as_deref(), Some("It is the entry point."));
        assert_eq!(
            learning(&app).in_flight_count(),
            0,
            "and it no longer counts as generating"
        );
    }

    /// The failure path takes the same route: a run that failed after its
    /// overlay closed reloads as failed, not as still thinking.
    #[test]
    fn a_failure_arriving_after_the_overlay_closed_is_still_saved() {
        let (_repo, _db, mut app) = opened_app_with_db();
        let id = app
            .learning_ask("What does this do?", LearningQaIntent::Explain, None)
            .unwrap();
        app.close_learning_mode();
        deliver(&mut app, &id, Err("Claude couldn't answer".to_string()));

        app.open_learning_mode(0, 0).unwrap();
        let row = learning(&app)
            .qa
            .iter()
            .find(|r| r.id == id)
            .unwrap()
            .clone();
        assert_eq!(row.status, crate::app::LearningQaStatus::Failed);
        let reason = row.error.as_deref().unwrap_or_default();
        assert!(reason.contains("couldn't answer"), "{reason}");
        assert!(
            !reason.contains("AMF stopped"),
            "a real failure keeps its own reason rather than being reconciled: {reason}"
        );
    }

    // ── branch-changes context ───────────────────────────────

    /// A repo whose branch changes one line deep inside a long file, so the
    /// diff hunk and the file are very different sizes.
    fn repo_with_a_small_change_in_a_big_file() -> TempDir {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "--initial-branch=main"]);
        git(repo.path(), &["config", "user.name", "AMF Test"]);
        git(repo.path(), &["config", "user.email", "amf@example.com"]);
        std::fs::create_dir_all(repo.path().join("src")).unwrap();
        let base: String = (0..BIG_FILE_LINES)
            .map(|i| format!("fn line_{i}() {{}}\n"))
            .collect();
        std::fs::write(repo.path().join("src/big.rs"), &base).unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "initial"]);
        git(repo.path(), &["checkout", "-b", "my-feat"]);
        let changed = base.replace("fn line_30() {}", "fn line_30_renamed() {}");
        std::fs::write(repo.path().join("src/big.rs"), changed).unwrap();
        git(repo.path(), &["commit", "-am", "rename line 30"]);
        repo
    }

    const BIG_FILE_LINES: usize = 60;

    /// Open the big-file repo in branch-changes scope with the changed file
    /// loaded and every addressable diff line selected.
    fn app_on_the_changed_file() -> (TempDir, App) {
        let repo = repo_with_a_small_change_in_a_big_file();
        let mut app = app_at(repo.path(), true);
        app.open_learning_mode(0, 0).unwrap();
        app.learning_toggle_scope();
        while learning(&app).content_path.as_deref() != Some("src/big.rs") {
            app.learning_select_next_entry();
        }
        (repo, app)
    }

    /// Browsing a diff must not narrow what the agent can see: the surrounding
    /// file is hydrated from the snapshot, so a whole-file anchor really is the
    /// whole file and a line anchor still has context around it.
    #[test]
    fn a_changed_file_carries_its_whole_file_not_just_the_hunks() {
        let (_repo, mut app) = app_on_the_changed_file();

        let state = learning(&app);
        assert_eq!(state.scope, BrowseScope::BranchChanges);
        assert_eq!(
            state.content.len(),
            BIG_FILE_LINES,
            "the snapshot's copy of the file, not only the diff rows"
        );
        assert!(
            state.selectable_line_count() < BIG_FILE_LINES,
            "the pane itself still addresses diff rows only"
        );

        // "Whole file" means the whole file, in this scope too.
        app.learning_select_whole_file();
        assert_eq!(
            app.learning_selection_text().lines().count(),
            BIG_FILE_LINES
        );

        // And a line selection gets surrounding context to sit in.
        app.learning_start_range();
        app.learning_cursor_move(1000);
        let ctx = app
            .learning_prompt_context("What changed here?", LearningQaIntent::Explain, Vec::new())
            .unwrap();
        assert_eq!(ctx.file_lines.len(), BIG_FILE_LINES);
        let prompt = build_prompt(&ctx);
        assert!(prompt.contains("Surrounding context"), "{prompt}");
        assert!(
            prompt.contains("fn line_0() {}"),
            "context reaches code the hunk never touched: {prompt}"
        );
    }

    /// A diff excerpt must stay readable *as* a diff: markers intact, and the
    /// prompt saying what they mean, so an addition and the line it replaced
    /// can't read as two adjacent source lines.
    #[test]
    fn a_diff_selection_keeps_its_markers_and_says_it_is_a_diff() {
        let (_repo, mut app) = app_on_the_changed_file();
        app.learning_start_range();
        app.learning_cursor_move(1000);

        let selection = app.learning_selection_text();
        assert!(
            selection
                .lines()
                .any(|l| l.starts_with("+fn line_30_renamed")),
            "the addition keeps its marker: {selection}"
        );
        assert!(
            selection.lines().any(|l| l.starts_with("-fn line_30()")),
            "and so does the line it replaced: {selection}"
        );

        let ctx = app
            .learning_prompt_context("What changed here?", LearningQaIntent::Explain, Vec::new())
            .unwrap();
        assert!(ctx.selection_is_diff);
        let prompt = build_prompt(&ctx);
        assert!(prompt.contains("unified diff"), "{prompt}");
        assert!(
            prompt.contains("+fn line_30_renamed() {}"),
            "quoted verbatim, with no line-number gutter to hide the marker: {prompt}"
        );
        assert!(
            !prompt.contains("--- The code they are asking about ---"),
            "a diff is never presented as plain source: {prompt}"
        );
    }

    /// Diff-ness is captured with the selection, not re-read at submit time:
    /// following up after browsing back to the repo tree must still label the
    /// parent's excerpt as a diff.
    #[test]
    fn a_follow_up_keeps_its_parents_diff_labelling_after_browsing_away() {
        let (_repo, mut app) = app_on_the_changed_file();
        app.learning_start_range();
        app.learning_cursor_move(1000);
        let parent = ask_and_answer(&mut app, "What changed here?", "A function was renamed.");
        assert!(
            learning(&app)
                .qa
                .iter()
                .find(|r| r.id == parent)
                .unwrap()
                .selection_is_diff
        );

        // Browse back to plain source before following up.
        app.learning_toggle_scope();
        assert_eq!(learning(&app).scope, BrowseScope::RepoTree);
        assert!(
            !learning(&app).selection_is_diff(),
            "the live cursor is on ordinary source now"
        );

        let child = follow_up(&mut app, "Why would you rename it?");
        let row = learning(&app).qa.iter().find(|r| r.id == child).unwrap();
        assert!(
            row.selection_is_diff,
            "the follow-up quotes its parent's diff, so it is still a diff"
        );
        assert!(row.selection_text.lines().any(|l| l.starts_with('+')));
    }
}
