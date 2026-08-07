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
    LearningListGroup, LearningQa, LearningViewState, Selection,
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
