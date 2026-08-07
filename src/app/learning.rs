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
        Ok(())
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
            Ok(rows) => rows,
            Err(e) => {
                self.log_error(
                    "learning",
                    format!("failed to load past questions: {e} (starting with an empty history)"),
                );
                Vec::new()
            }
        }
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
            BrowseScope::BranchChanges => {
                match crate::diff::load_snapshot(&workdir, None, false) {
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
                }
            }
            BrowseScope::RepoTree => {
                let files = if is_git {
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
            AppMode::Learning(state) => state.selected_entry().and_then(|e| e.path()).map(str::to_string),
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
        let Some((entry, workdir, scope)) = (match &self.mode {
            AppMode::Learning(state) => state
                .selected_entry()
                .cloned()
                .map(|e| (e, state.workdir.clone(), state.scope)),
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
                // In branch-changes scope the diff itself is the content; the
                // file on disk is only read for repo-tree browsing.
                let loaded = match scope {
                    BrowseScope::BranchChanges => Ok(Vec::new()),
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

/// Read a file for the content pane, or say why it can't be shown. The message
/// is user-facing, so it names the limit rather than the errno.
pub fn load_file_lines(path: &Path) -> Result<Vec<String>, String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("Couldn't open {}: {e}", path.display()))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "This file is {} — too big to show here (the limit is {} MB).",
            human_bytes(meta.len()),
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    if looks_binary(&bytes) {
        return Err("This looks like a binary file, so there's nothing to read here.".to_string());
    }
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.lines().map(ToOwned::to_owned).collect())
}

/// A NUL byte in the first few KB is the same heuristic git uses.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .take(BINARY_SNIFF_BYTES)
        .any(|byte| *byte == 0)
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
            let end = lines.get(to.min(lines.len() - 1)).and_then(diff_line_number);
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
        .or(if hunk_starts.is_empty() { None } else { Some(0) })
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
pub fn selection_text(state: &LearningViewState) -> String {
    match state.anchor {
        // The project anchor has no text: the question is about the repo.
        LearningAnchor::Project => String::new(),
        LearningAnchor::File => match state.scope {
            BrowseScope::RepoTree => state.content.join("\n"),
            BrowseScope::BranchChanges => state
                .selected_diff_file()
                .map(|f| f.addressable_line_texts().join("\n"))
                .unwrap_or_default(),
        },
        LearningAnchor::Hunk { index } => {
            let Some(file) = state.selected_diff_file() else {
                return String::new();
            };
            let texts = file.addressable_line_texts();
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
                    let texts = file.addressable_line_texts();
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
    /// The whole file the selection came from, for surrounding context.
    pub file_lines: Vec<String>,
    /// 1-based line the selection starts at, when it has one.
    pub selection_start_line: Option<usize>,
    pub question: String,
    pub intent: LearningQaIntent,
    pub level: LearningLevel,
    /// Oldest first. Trimmed to [`MAX_FOLLOW_UP_DEPTH`] by the builder.
    pub ancestors: Vec<ParentTurn>,
}

/// Build the prompt for one question.
///
/// Structure is fixed: who and where, then what they're looking at, then any
/// earlier turns, then the question, then the instructions selected by intent
/// and level. Instructions come last so they're the freshest thing the model
/// reads.
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
        out.push_str(
            "Their question is about the project as a whole, not about one file.\n\n",
        );
    } else if !ctx.selection_text.trim().is_empty() {
        out.push_str("--- The code they are asking about ---\n");
        out.push_str(&numbered_block(
            &ctx.selection_text,
            ctx.selection_start_line.unwrap_or(1),
            MAX_SELECTION_LINES,
        ));
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
    out
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
    if matches!(
        ctx.anchor,
        LearningAnchor::Project | LearningAnchor::File
    ) {
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

impl App {
    /// Assemble the prompt context for a question asked right now, against the
    /// overlay's current anchor.
    pub fn learning_prompt_context(
        &self,
        question: &str,
        intent: LearningQaIntent,
        ancestors: Vec<ParentTurn>,
    ) -> Option<LearningPromptContext> {
        let AppMode::Learning(state) = &self.mode else {
            return None;
        };
        let selection_start_line = match state.anchor {
            LearningAnchor::Lines { start, .. } => Some(start),
            LearningAnchor::Hunk { .. } => match anchor_for_cursor(state) {
                LearningAnchor::Lines { start, .. } => Some(start),
                _ => None,
            },
            _ => Some(1),
        };
        Some(LearningPromptContext {
            project_name: state.project_name.clone(),
            feature_name: state.feature_name.clone(),
            file_path: match state.anchor {
                LearningAnchor::Project => None,
                _ => state.content_path.clone(),
            },
            anchor: state.anchor,
            selection_text: selection_text(state),
            file_lines: state.content.clone(),
            selection_start_line,
            question: question.to_string(),
            intent,
            level: state.level,
            ancestors,
        })
    }
}

// ── asking (headless, non-blocking) ──────────────────────────

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
        let question = question.trim().to_string();
        if question.is_empty() {
            return None;
        }
        let ancestors = self.learning_ancestor_turns(parent_qa_id.as_deref());
        let ctx = self.learning_prompt_context(&question, intent, ancestors)?;

        let AppMode::Learning(state) = &mut self.mode else {
            return None;
        };
        let (line_start, _) = state.anchor.line_range();
        let _ = line_start;
        let qa = LearningQa {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: state.session_id.clone(),
            parent_qa_id,
            file_path: ctx.file_path.clone(),
            anchor: state.anchor,
            selection_text: ctx.selection_text.clone(),
            question: question.clone(),
            intent,
            level: state.level,
            answer: None,
            harness: state.harness.clone(),
            run_mode: crate::app::LearningRunMode::NoTools,
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
        state.qa.push(qa.clone());
        // Show the newest question, so an answer that takes a while is visibly
        // *this* question's answer.
        state.selected_qa = state.qa.len() - 1;

        self.persist_learning_qa(&qa);
        self.spawn_learning_run(
            &qa_id,
            harness,
            workdir,
            build_prompt(&ctx),
            crate::app::LearningRunMode::NoTools,
        );
        Some(qa_id)
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
            match answer.result {
                Ok(text) => {
                    let text = text.trim().to_string();
                    if let AppMode::Learning(state) = &mut self.mode
                        && let Some(row) = state.qa.iter_mut().find(|r| r.id == answer.qa_id)
                    {
                        row.answer = Some(text);
                        row.status = crate::app::LearningQaStatus::Answered;
                        row.error = None;
                        row.updated_at = crate::db::learning::now_timestamp();
                    }
                }
                Err(message) => {
                    self.log_error("learning", format!("question failed: {message}"));
                    if let AppMode::Learning(state) = &mut self.mode
                        && let Some(row) = state.qa.iter_mut().find(|r| r.id == answer.qa_id)
                    {
                        row.status = crate::app::LearningQaStatus::Failed;
                        row.error = Some(message);
                        row.updated_at = crate::db::learning::now_timestamp();
                    }
                }
            }
            self.persist_learning_qa_by_id(&answer.qa_id);
        }
        changed
    }

    /// The chain of earlier turns leading to `parent_qa_id`, oldest first.
    /// Only answered rows are carried — an unanswered parent has no context to
    /// give.
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
            current = row.parent_qa_id.clone();
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

// ── session settings (level, harness) ────────────────────────

impl App {
    /// Flip between newcomer and familiar answers. Applies to later questions
    /// only — an answer already on screen is never rewritten under the user.
    pub fn learning_toggle_level(&mut self) {
        let Some((session_id, harness, level)) = (match &mut self.mode {
            AppMode::Learning(state) => {
                state.level = state.level.toggled();
                Some((
                    state.session_id.clone(),
                    state.harness.clone(),
                    state.level,
                ))
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
mod tests {
    use super::*;
    use crate::app::{ProjectStatus, ProjectStore};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use crate::project::{Feature, Project, VibeMode};
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
        std::fs::write(repo.path().join("src/main.rs"), "fn main() {\n    ok();\n}\n").unwrap();
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
            file_lines: (1..=10).map(|i| format!("line {i}")).collect(),
            selection_start_line: Some(3),
            question: "What does this do?".to_string(),
            intent: LearningQaIntent::Explain,
            level: LearningLevel::Newcomer,
            ancestors: Vec::new(),
        }
    }

    #[test]
    fn every_prompt_carries_identity_path_numbered_selection_and_context() {
        let prompt = build_prompt(&sample_context());

        assert!(prompt.contains("Project: my-project"), "{prompt}");
        assert!(prompt.contains("Branch / feature: learning-mode"), "{prompt}");
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

    #[test]
    fn the_newcomer_overlay_is_present_by_default_and_absent_when_familiar() {
        let newcomer = build_prompt(&sample_context());
        assert!(newcomer.contains("Define every technical term"), "{newcomer}");
        assert!(newcomer.contains("Where to look next"), "{newcomer}");
        assert!(newcomer.contains("No question is too basic"), "{newcomer}");

        let mut ctx = sample_context();
        ctx.level = LearningLevel::Familiar;
        let familiar = build_prompt(&ctx);
        assert!(!familiar.contains("Define every technical term"), "{familiar}");
        assert!(
            !familiar.contains("Finish with a section headed"),
            "{familiar}"
        );
        assert!(familiar.contains("Be dense and skip the basics"), "{familiar}");
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
        assert!(
            prompt.contains("about the project as a whole"),
            "{prompt}"
        );
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
        let msg = headless_failure_message(
            &AgentKind::Claude,
            &anyhow::anyhow!("claude CLI not found"),
        );
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
            learning(&app).qa.iter().find(|r| r.id == id).unwrap().harness,
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
        assert!(!state
            .entries
            .iter()
            .any(|e| matches!(e, LearningListEntry::StartHereHeader)));

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
}

