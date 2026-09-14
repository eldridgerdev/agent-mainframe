use crate::app::{
    App, AppMode, BrowseScope, LearningAnchor, LearningAnchorDrift, LearningAnchorLoss,
    LearningFocus, LearningListEntry, LearningListGroup, LearningQa, LearningViewState,
};
use crate::diff::{DiffFile, DiffLineLocation};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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
pub(super) const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// How much of a file is sniffed for a NUL byte before calling it binary.
pub(super) const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Safety valve on the repo-tree file list. This used to be the *browsing*
/// limit at 20,000, back when the list was flat and every path in the repo was
/// a row: past that the pane was unusable anyway, so capping the listing and
/// capping what you could reach were the same decision. A tree only emits rows
/// for what is expanded, so the two have come apart — reachability is now
/// bounded per directory by `MAX_DIR_CHILDREN`, and this is only here to stop a
/// pathological repository from being read into memory whole. It is deliberately
/// far above any real project.
pub(super) const MAX_REPO_ENTRIES: usize = 200_000;

/// Cap on the rows one directory contributes. This is the limit that actually
/// bites, and it bites where the user can see it: a directory over the cap says
/// how many children it is not showing, rather than the whole listing claiming
/// to be complete. A single directory with thousands of entries is unbrowsable
/// however it is rendered, and is nearly always generated output.
pub(super) const MAX_DIR_CHILDREN: usize = 2_000;

/// Depth cap for the non-git fallback walk.
pub(super) const MAX_WALK_DEPTH: usize = 12;

/// Directories the non-git fallback walk skips. Git projects get `.gitignore`
/// handling for free from `git ls-files`; a non-git project has no ignore
/// rules at all, so this short list stands in for the obvious noise.
pub(super) const WALK_SKIP_DIRS: &[&str] = &[
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

impl App {
    /// Switch between "all files in this project" and "files changed on this
    /// branch", reloading the list in place. Branch-changes scope needs git,
    /// so a non-git project stays where it is and says why.
    pub fn learning_toggle_scope(&mut self) {
        let is_git = match &self.mode {
            AppMode::Learning(state) => state.is_git,
            _ => return,
        };
        let stuck_in_repo_tree = matches!(
            &self.mode,
            AppMode::Learning(state) if state.scope == BrowseScope::RepoTree && !is_git
        );
        if stuck_in_repo_tree {
            // There is no second scope to switch to, but the key still has a
            // job here: rebuild the list in place. Elsewhere the advice for a
            // file that vanished since the list was built is "press s" — and
            // if this branch only explained itself and returned, that advice
            // would be a no-op in exactly the projects it's aimed at.
            self.learning_reload_entries();
            self.learning_load_selected_content();
            if let AppMode::Learning(state) = &mut self.mode
                // A problem listing the files is more useful than the reminder
                // that this isn't a repository, so it keeps the line.
                && state.error.is_none()
            {
                state.error = Some(
                    "This project isn't a git repository, so there are no branch changes to \
                     show — rebuilt the file list instead."
                        .to_string(),
                );
            }
            return;
        }
        if let AppMode::Learning(state) = &mut self.mode {
            state.scope = state.scope.toggled();
            state.selected_entry = 0;
            state.list_scroll = 0;
            state.error = None;
        }
        self.learning_reload_entries();
        self.learning_load_selected_content();
    }
}

impl App {
    /// Rebuild the file list for the current scope.
    pub fn learning_reload_entries(&mut self) {
        let Some((scope, workdir, is_git, has_history, mut expanded, expanded_seeded)) =
            (match &self.mode {
                AppMode::Learning(state) => Some((
                    state.scope,
                    state.workdir.clone(),
                    state.is_git,
                    !state.qa.is_empty(),
                    state.expanded_dirs.clone(),
                    state.expanded_seeded,
                )),
                _ => None,
            })
        else {
            return;
        };

        let mut load_error: Option<String> = None;
        let mut unreadable: Vec<String> = Vec::new();
        let mut diff_files: Vec<DiffFile> = Vec::new();
        let mut repo_files: Vec<String> = Vec::new();
        let mut start_here_files: Vec<String> = Vec::new();
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
                    let walk = walk_files_capped(&workdir, MAX_REPO_ENTRIES, MAX_WALK_DEPTH);
                    unreadable = walk.unreadable;
                    walk.files
                };
                if let Some(total) = cap_repo_entries(&mut files, MAX_REPO_ENTRIES) {
                    load_error.get_or_insert(format!(
                        "This project has {total} files — showing the first {MAX_REPO_ENTRIES}. \
                         Switch to branch changes to see what's actually changed."
                    ));
                }
                // A subtree that couldn't be opened leaves no trace in the
                // list, so say it out loud: "this project has no such
                // directory" and "AMF couldn't read it" look identical
                // otherwise, and only one of them is true.
                if !unreadable.is_empty() {
                    load_error.get_or_insert(format!(
                        "{} folder(s) here couldn't be read, so anything inside them is missing \
                         from this list. See the debug log (D on the dashboard) for which.",
                        unreadable.len()
                    ));
                }
                let start_here = if has_history {
                    Vec::new()
                } else {
                    start_here_candidates(&workdir)
                };
                // First listing of this project: open the path down to the
                // orientation files so the tree arrives useful rather than
                // shut. Seeded once and never re-applied — after that the tree
                // is the user's, and a reload that re-opened what they closed
                // would be the overlay arguing with them.
                if !expanded_seeded {
                    expanded.extend(default_expanded_dirs(&start_here));
                }
                repo_files = files;
                start_here_files = start_here;
                Vec::new()
            }
        };

        if let AppMode::Learning(state) = &mut self.mode {
            state.diff_files = diff_files;
            if scope == BrowseScope::RepoTree {
                state.repo_files = repo_files;
                state.start_here = start_here_files;
                state.expanded_dirs = expanded;
                state.expanded_seeded = true;
            } else {
                state.entries = entries;
            }
            if state.selected_entry >= state.entries.len() {
                state.selected_entry = state.entries.len().saturating_sub(1);
            }
            state.error = load_error.clone();
        }
        // Repo-tree rows come from the cached listing, so building them is the
        // same step a collapse takes — one function, not two that have to be
        // kept agreeing.
        if scope == BrowseScope::RepoTree {
            let overflow = self.learning_rebuild_tree();
            if overflow > 0 {
                let msg = format!(
                    "{overflow} more item(s) sit at the top level than this list shows \
                     (the limit is {MAX_DIR_CHILDREN} per folder). Open a folder to browse \
                     inside it, or press s for just this branch's changes."
                );
                if let AppMode::Learning(state) = &mut self.mode {
                    state.error.get_or_insert(msg.clone());
                }
                if load_error.is_none() {
                    load_error = Some(msg);
                }
            }
        }
        if let Some(msg) = load_error {
            self.log_warn("learning", msg);
        }
        for dir in unreadable {
            self.log_warn("learning", format!("couldn't read the folder {dir}"));
        }
    }
}

impl App {
    /// Rebuild the repo-tree rows from the cached listing. No disk access:
    /// expanding a directory must not re-run `git ls-files`, which on a large
    /// repository would make the tree slower than the flat list it replaced.
    /// Returns the root-level overflow, which only the caller that reads the
    /// listing has anywhere to report.
    pub(super) fn learning_rebuild_tree(&mut self) -> usize {
        let AppMode::Learning(state) = &mut self.mode else {
            return 0;
        };
        if state.scope != BrowseScope::RepoTree {
            return 0;
        }
        let (rows, overflow) = build_repo_tree_entries(
            &state.repo_files,
            &state.start_here,
            state.start_here_collapsed,
            &state.expanded_dirs,
        );
        state.entries = rows;
        if state.selected_entry >= state.entries.len() {
            state.selected_entry = state.entries.len().saturating_sub(1);
        }
        overflow
    }
}

impl App {
    /// Rebuild the list and put the cursor back on the row it was on. Every
    /// tree operation is "change `expanded_dirs`, then this": the rows are
    /// derived, so the cursor is an index into something that no longer exists
    /// by the time the new list is built.
    ///
    /// A directory keeps the cursor by its own path; a file keeps it by its
    /// path too, and if that file is no longer listed (its folder was just
    /// closed) the cursor falls back to the nearest ancestor directory still
    /// on screen — the row that swallowed it — rather than to wherever the old
    /// index happens to land.
    pub(super) fn learning_rebuild_keeping_cursor(&mut self) {
        let key = match &self.mode {
            AppMode::Learning(state) => state
                .selected_entry()
                .and_then(|e| e.row_key())
                .map(|(is_dir, path)| (is_dir, path.to_string())),
            _ => return,
        };
        self.learning_rebuild_tree();
        let Some((was_dir, path)) = key else { return };
        if let AppMode::Learning(state) = &mut self.mode {
            let exact = state.entries.iter().position(|e| {
                e.row_key()
                    .is_some_and(|(d, p)| d == was_dir && p == path.as_str())
            });
            if let Some(idx) = exact {
                state.selected_entry = idx;
                return;
            }
            // The row is gone, so it was inside something that just closed.
            // Walk up its path until a directory row exists.
            let mut prefix = path.as_str();
            while let Some(cut) = prefix.rfind('/') {
                prefix = &prefix[..cut];
                if let Some(idx) = state
                    .entries
                    .iter()
                    .position(|e| e.dir_path() == Some(prefix))
                {
                    state.selected_entry = idx;
                    return;
                }
            }
        }
    }
}

impl App {
    /// Open or close the directory under the cursor. No-op with a spoken
    /// reason elsewhere — on a file this is routed to `learning_jump_to_parent`
    /// by the key handler, so this only ever sees a directory.
    pub fn learning_toggle_dir(&mut self) -> bool {
        let Some(path) = (match &self.mode {
            AppMode::Learning(state) => state
                .selected_entry()
                .and_then(|e| e.dir_path())
                .map(str::to_string),
            _ => None,
        }) else {
            return false;
        };
        if let AppMode::Learning(state) = &mut self.mode
            && !state.expanded_dirs.remove(&path)
        {
            state.expanded_dirs.insert(path);
        }
        self.learning_rebuild_keeping_cursor();
        true
    }
}

impl App {
    /// Tree movement only ever means something in the file list. Pressing it
    /// while reading the content or the history is a wrong guess about which
    /// pane has the cursor, so it is answered rather than swallowed — and never
    /// allowed to move a cursor the user cannot see.
    pub(super) fn learning_require_file_list_focus(&mut self) -> bool {
        let focused = match &self.mode {
            AppMode::Learning(state) => state.focus == LearningFocus::FileList,
            _ => return false,
        };
        if !focused {
            self.learning_notice("That moves the file list, which isn't focused — press Tab.");
        }
        focused
    }
}

impl App {
    /// `l` / `Right`: open the folder under the cursor, step into an already
    /// open one, or — on a file — do what `Enter` does.
    pub fn learning_expand_or_open(&mut self) {
        if !self.learning_require_file_list_focus() {
            return;
        }
        let state_kind = match &self.mode {
            AppMode::Learning(state) => match state.selected_entry() {
                Some(LearningListEntry::Dir { expanded, .. }) => Some(*expanded),
                _ => None,
            },
            _ => return,
        };
        match state_kind {
            Some(false) => {
                self.learning_toggle_dir();
            }
            // Already open, so the useful move is into it. The first child is
            // always the next row — that is what flattening guarantees.
            Some(true) => self.learning_select_next_entry(),
            None => self.learning_activate_selection(),
        }
    }
}

impl App {
    /// `h` / `Left`: close the folder under the cursor, or step out to the one
    /// containing this row.
    pub fn learning_collapse_or_parent(&mut self) {
        if !self.learning_require_file_list_focus() {
            return;
        }
        let on_open_dir = matches!(
            &self.mode,
            AppMode::Learning(state)
                if matches!(
                    state.selected_entry(),
                    Some(LearningListEntry::Dir { expanded: true, .. })
                )
        );
        if on_open_dir {
            self.learning_toggle_dir();
        } else {
            self.learning_jump_to_parent();
        }
    }
}

impl App {
    /// Move the cursor to the directory containing the current row, closing
    /// nothing. Two rows have nowhere to go: a top-level one, and one whose
    /// folder has no row of its own (the flat branch-changes list, or a pinned
    /// `Start here` file whose folder the root cap dropped). Each says so
    /// rather than being swallowed.
    pub fn learning_jump_to_parent(&mut self) {
        let Some(path) = (match &self.mode {
            AppMode::Learning(state) => state
                .selected_entry()
                .and_then(|e| e.row_key())
                .map(|(_, p)| p.to_string()),
            _ => None,
        }) else {
            return;
        };
        let Some(cut) = path.rfind('/') else {
            self.learning_notice("This is already at the top level of the project.");
            return;
        };
        let parent = path[..cut].to_string();
        let found = match &mut self.mode {
            AppMode::Learning(state) => {
                match state
                    .entries
                    .iter()
                    .position(|e| e.dir_path() == Some(parent.as_str()))
                {
                    Some(idx) => {
                        state.selected_entry = idx;
                        true
                    }
                    // A pinned `Start here` file, or any row in the flat
                    // branch-changes list, can sit on screen while the folder
                    // holding it has no row at all.
                    None => false,
                }
            }
            _ => return,
        };
        if found {
            return;
        }
        let flat = matches!(
            &self.mode,
            AppMode::Learning(state) if state.scope != BrowseScope::RepoTree
        );
        if flat {
            self.learning_notice(
                "Changed files are listed flat, without folders — press s for the project tree.",
            );
        } else {
            self.learning_notice(format!(
                "{parent}/ isn't open in the tree, so there's nowhere to step out to — press Z to open every folder."
            ));
        }
    }
}

impl App {
    /// Open every directory that has one, or shut the tree back to its top
    /// level. One key, because the useful gesture is "show me everything" and
    /// its undo — and because the footer cannot afford two.
    pub fn learning_toggle_expand_all(&mut self) {
        let expand = match &self.mode {
            // Anything still closed means the gesture is "open it all"; only a
            // fully open tree collapses.
            AppMode::Learning(state) => state.entries.iter().any(|e| {
                matches!(
                    e,
                    LearningListEntry::Dir {
                        expanded: false,
                        ..
                    }
                )
            }),
            _ => return,
        };
        if let AppMode::Learning(state) = &mut self.mode {
            if expand {
                // Taken from the cached path list, not from the rows: the
                // directories being opened are precisely the ones with no rows
                // yet, so walking the visible tree would only ever open one
                // level per press.
                state.expanded_dirs = all_dir_paths(&state.repo_files);
            } else {
                state.expanded_dirs.clear();
            }
        }
        self.learning_rebuild_keeping_cursor();
        self.learning_notice(if expand {
            "Opened every folder. Press Z again to fold them."
        } else {
            "Folded every folder. Enter opens one."
        });
    }
}

impl App {
    /// Raise a plain confirmation on the overlay's shared banner line. Unlike
    /// `learning_notice_for_qa` this one isn't about a Q&A row, so it carries
    /// no row id and survives the history cursor moving.
    pub(super) fn learning_notice(&mut self, message: impl Into<String>) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.notice = Some(message.into());
            state.notice_qa_id = None;
            state.error = None;
        }
    }
}

impl App {
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
        self.learning_rebuild_tree();
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
}

impl App {
    pub fn learning_select_next_entry(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            if state.entries.is_empty() {
                return;
            }
            state.selected_entry = (state.selected_entry + 1) % state.entries.len();
        }
        self.learning_load_selected_content();
    }
}

impl App {
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
}

impl App {
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
            // A directory is navigation, not a selection. Resting on one
            // deliberately leaves the loaded file and the anchor alone, so
            // walking down to `src/app/learning.rs` never quietly drops the
            // question you had lined up two folders ago.
            LearningListEntry::Dir { .. } => {}
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
                    BrowseScope::RepoTree => load_file_lines(&workdir.join(&path), &path),
                };
                // Logged out here rather than inside the borrow: a file that
                // won't open is the one failure the user meets while simply
                // moving the cursor, so it belongs in the debug log too.
                let load_failure = loaded
                    .as_ref()
                    .err()
                    .map(|reason| format!("couldn't load {path} for browsing: {reason}"));
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
                if let Some(msg) = load_failure {
                    self.log_warn("learning", msg);
                }
            }
        }
    }
}

impl App {
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
}

impl App {
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
}

impl App {
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
}

impl App {
    /// Anchor the next question to the whole current file.
    pub fn learning_select_whole_file(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode
            && state.content_path.is_some()
        {
            state.selection_anchor = None;
            state.anchor = LearningAnchor::File;
        }
    }
}

impl App {
    /// Anchor the next question to the project as a whole.
    pub fn learning_select_project(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.selection_anchor = None;
            state.anchor = LearningAnchor::Project;
        }
    }
}

impl App {
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
}

impl App {
    /// The text the current anchor covers, captured verbatim onto a Q&A row so
    /// the answer stays readable after the file moves on. Test-only sugar over
    /// the free [`selection_text`] helper, which is what production uses.
    #[cfg(test)]
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
        .filter(|candidate| {
            let path = workdir.join(candidate);
            // `is_file` alone matches both README.md and readme.md on a
            // case-insensitive filesystem. Match the stored entry name so
            // the reading list uses its actual spelling and lists it once.
            path.parent()
                .and_then(|parent| std::fs::read_dir(parent).ok())
                .is_some_and(|entries| {
                    entries.filter_map(Result::ok).any(|entry| {
                        Some(entry.file_name().as_os_str()) == path.file_name()
                            && entry.path().is_file()
                    })
                })
        })
        .map(|candidate| (*candidate).to_string())
        .collect()
}

/// The repo-tree file list: the pinned orientation group (when it has any
/// members and hasn't been collapsed), then the repo as a collapsible tree.
///
/// Returns the rows plus the root-level overflow `flatten_tree` reports, which
/// has no row of its own to be stated on.
pub fn build_repo_tree_entries(
    files: &[String],
    start_here: &[String],
    collapsed: bool,
    expanded: &BTreeSet<String>,
) -> (Vec<LearningListEntry>, usize) {
    let mut entries = Vec::with_capacity(start_here.len() + 2);
    if !start_here.is_empty() {
        entries.push(LearningListEntry::StartHereHeader);
        if !collapsed {
            entries.push(LearningListEntry::ProjectTour);
            for path in start_here {
                // The orientation group is a reading list, not a tree: these
                // are shortcuts to files that also appear in their real place
                // below, so they stay flat at depth 0.
                entries.push(LearningListEntry::File {
                    path: path.clone(),
                    group: LearningListGroup::StartHere,
                    diff_index: None,
                    depth: 0,
                });
            }
        }
    }
    let (tree, root_overflow) = flatten_tree(files, expanded);
    entries.extend(tree);
    (entries, root_overflow)
}

/// One directory while the tree is being assembled. `BTreeMap`/sort do the
/// ordering, so the flatten step never sorts.
#[derive(Default)]
pub(super) struct TreeNode {
    dirs: BTreeMap<String, TreeNode>,
    /// Leaf names only — the full path is rebuilt on the way down.
    files: Vec<String>,
    /// Files anywhere beneath here, so a collapsed row can say what it hides.
    file_count: usize,
}

impl TreeNode {
    fn insert(&mut self, path: &str) {
        let mut node = self;
        let mut parts = path.split('/').peekable();
        while let Some(part) = parts.next() {
            node.file_count += 1;
            if parts.peek().is_none() {
                node.files.push(part.to_string());
                return;
            }
            node = node.dirs.entry(part.to_string()).or_default();
        }
    }
}

/// Flatten a path list into tree rows: at each level, directories before files,
/// each in name order, descending only into directories listed in `expanded`.
///
/// This is the whole of Epic 7's ordering decision, and it is a pure function
/// over the path list so the ordering, depth, and collapse behaviour are
/// testable without an overlay.
///
/// Returns the rows plus how many *root-level* children were dropped by
/// `MAX_DIR_CHILDREN`. Every other directory reports its own overflow on its
/// row; the root has no row to report on, so it comes back here for the banner.
pub fn flatten_tree(
    files: &[String],
    expanded: &BTreeSet<String>,
) -> (Vec<LearningListEntry>, usize) {
    let mut root = TreeNode::default();
    for path in files {
        root.insert(path);
    }
    let root_children = root.dirs.len() + root.files.len();
    let mut out = Vec::new();
    push_level(&mut root, "", 0, expanded, &mut out);
    (out, root_children.saturating_sub(MAX_DIR_CHILDREN))
}

pub(super) fn push_level(
    node: &mut TreeNode,
    prefix: &str,
    depth: usize,
    expanded: &BTreeSet<String>,
    out: &mut Vec<LearningListEntry>,
) {
    // Directories first so structure reads before contents: a newcomer
    // scanning `src/` wants to see that `app/` exists before wading through
    // the twenty files sitting beside it.
    let mut budget = MAX_DIR_CHILDREN;

    for (name, child) in node.dirs.iter_mut() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        let path = join_path(prefix, name);
        let is_expanded = expanded.contains(&path);
        // Truncation is reported on the row that truncated, so it is computed
        // here where the child's own budget is known.
        let child_children = child.dirs.len() + child.files.len();
        out.push(LearningListEntry::Dir {
            path: path.clone(),
            depth,
            expanded: is_expanded,
            file_count: child.file_count,
            truncated: child_children.saturating_sub(MAX_DIR_CHILDREN),
        });
        if is_expanded {
            push_level(child, &path, depth + 1, expanded, out);
        }
    }

    node.files.sort();
    for name in node.files.iter() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        out.push(LearningListEntry::File {
            path: join_path(prefix, name),
            group: LearningListGroup::Files,
            diff_index: None,
            depth,
        });
    }
}

pub(super) fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

/// The directories to open when the overlay first lists a repository:
/// every ancestor of a `Start here` candidate, and nothing else.
///
/// The plan weighed two honest defaults and rejected both. Everything expanded
/// is the flat wall of rows the tree exists to replace; everything collapsed is
/// structurally honest but puts `src/` — the only directory most newcomers want
/// — behind a keypress they have to guess at. This is the middle: the top level
/// is visible, and the path down to the files the orientation group already
/// decided were worth reading is open, so `src/main.rs` is on screen at open.
pub fn default_expanded_dirs(start_here: &[String]) -> BTreeSet<String> {
    let mut dirs = BTreeSet::new();
    for path in start_here {
        let mut prefix = String::new();
        // The last component is the file itself, so it is not a directory.
        let parts: Vec<&str> = path.split('/').collect();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            prefix = join_path(&prefix, part);
            dirs.insert(prefix.clone());
        }
    }
    dirs
}

/// Every directory that appears anywhere in a path list — what "expand all"
/// expands. Derived from the paths rather than from the rows on screen, since
/// a closed directory contributes no rows and is exactly what is being opened.
pub fn all_dir_paths(files: &[String]) -> BTreeSet<String> {
    let mut dirs = BTreeSet::new();
    for path in files {
        let mut prefix = String::new();
        let parts: Vec<&str> = path.split('/').collect();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            prefix = join_path(&prefix, part);
            dirs.insert(prefix.clone());
        }
    }
    dirs
}

/// The branch-changes file list. No orientation group here: the user already
/// knows what they're looking for when they're reading their own diff — and no
/// tree, because a handful of changed files needs no structure and the paths
/// are the point.
pub fn build_changed_entries(files: &[DiffFile]) -> Vec<LearningListEntry> {
    files
        .iter()
        .enumerate()
        .map(|(i, file)| LearningListEntry::File {
            path: file.path.clone(),
            group: LearningListGroup::Files,
            diff_index: Some(i),
            depth: 0,
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
/// `label` is what the message calls the file — the repo-relative path, not
/// `path` itself. A workdir prefix is both noise (the pane title already names
/// the file) and long enough to push the actual advice off the end of the
/// line, which is how it read the first time this was captured.
pub fn load_file_lines(path: &Path, label: &str) -> Result<Vec<String>, String> {
    let meta = std::fs::metadata(path).map_err(|e| {
        format!(
            "Couldn't open {label}: {e}. It may have been moved or deleted since this list \
             was built — press s to rebuild the list (twice, if that switches scope), \
             or pick another file."
        )
    })?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "This file is {} — too big to show here (the limit is {} MB). Pick another file, \
             or press P to ask about the project as a whole.",
            human_bytes(meta.len()),
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| {
        format!(
            "Couldn't read {label}: {e}. Check you have permission to read it, \
             or pick another file."
        )
    })?;
    if looks_binary(&bytes) {
        return Err(
            "This looks like a binary file, so there's nothing to read here. \
             Pick a source file from the list instead."
                .to_string(),
        );
    }
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.lines().map(ToOwned::to_owned).collect())
}

/// A NUL byte in the first few KB is the same heuristic git uses.
pub(super) fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0)
}

pub(super) fn human_bytes(len: u64) -> String {
    if len >= 1024 * 1024 {
        format!("{:.1} MB", len as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", len as f64 / 1024.0)
    }
}

/// A non-git listing, plus the directories the walk couldn't open.
///
/// The skipped directories are carried rather than dropped because their
/// absence is invisible: a listing missing a whole subtree looks exactly like a
/// project that doesn't have one, and this mode's user has no way to know
/// better.
pub struct RepoWalk {
    pub files: Vec<String>,
    pub unreadable: Vec<String>,
}

/// Depth- and entry-capped walk for projects git doesn't know about. There are
/// no ignore rules to inherit here, so [`WALK_SKIP_DIRS`] stands in for them.
pub fn walk_files_capped(root: &Path, max_entries: usize, max_depth: usize) -> RepoWalk {
    let mut out = Vec::new();
    let mut unreadable = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= max_entries || depth > max_depth {
            continue;
        }
        let read = match std::fs::read_dir(&dir) {
            Ok(read) => read,
            Err(e) => {
                unreadable.push(format!(
                    "{}: {e}",
                    dir.strip_prefix(root).unwrap_or(&dir).display()
                ));
                continue;
            }
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
    unreadable.sort();
    RepoWalk {
        files: out,
        unreadable,
    }
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
pub(super) fn diff_line_number(loc: &DiffLineLocation) -> Option<usize> {
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
pub(super) fn hunk_span(file: &DiffFile, index: usize) -> Option<(usize, usize)> {
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

// ── anchor drift ─────────────────────────────────────────────

/// The file a stored anchor points at, as it stands now.
pub(super) enum AnchorTarget {
    /// The file is not in the working directory any more.
    Gone,
    /// It is there, but this overlay can't read it (too big, binary, no
    /// permission). Nothing can be checked, and nothing is claimed.
    Unreadable,
    Lines(Vec<String>),
}

/// The lines a stored selection expects to still find in the file.
///
/// A plain-source selection is its own lines. A *diff* selection carries `+` /
/// `-` / ` ` markers, so the markers come off and the removed rows go with them
/// — those lines are precisely the ones that are not in the file. Every line is
/// trimmed, because re-indenting a file is drift the user does not want
/// reported and is the single most common way a stored range moves without the
/// code changing at all.
///
/// Blank lines are dropped rather than matched: they carry no evidence, and a
/// selection that opens or closes on one would otherwise anchor on whitespace.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ExpectedBlock {
    pub(super) lines: Vec<String>,
    /// How many lines were dropped *before* the first kept one. The stored
    /// range starts at the selection's first line, which may well be one of
    /// those, so this is the distance between "where the question was asked"
    /// and "where the evidence starts" — without it an unchanged selection
    /// that opens on a blank line reads as having moved down by exactly this
    /// many rows.
    pub(super) lead_offset: usize,
}

pub(super) fn expected_block(selection_text: &str, is_diff: bool) -> ExpectedBlock {
    fn kept(line: &str, is_diff: bool) -> Option<&str> {
        let text = if is_diff {
            match line.chars().next() {
                Some('+') | Some(' ') => &line[1..],
                // A removed row, or a `\ No newline` marker: not in the file.
                _ => return None,
            }
        } else {
            line
        };
        let text = text.trim();
        (!text.is_empty()).then_some(text)
    }
    let lead_offset = selection_text
        .lines()
        .take_while(|line| kept(line, is_diff).is_none())
        .count();
    ExpectedBlock {
        lines: selection_text
            .lines()
            .filter_map(|line| kept(line, is_diff))
            .map(ToOwned::to_owned)
            .collect(),
        lead_offset,
    }
}

/// Where a block of lines sits in `lines` now, relative to where it was stored.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum BlockMatch {
    /// Found at the stored position — nothing to report.
    AsStored,
    Moved {
        start: usize,
        end: usize,
    },
    NotFound,
    /// Found in more than one place. Guessing which is the original is exactly
    /// the kind of quiet wrongness this whole check exists to remove, so it is
    /// reported as a loss instead.
    Ambiguous,
}

/// Find `block` in `lines`, comparing trimmed text and ignoring blank lines, so
/// that a re-indent or an added blank line is not read as movement.
///
/// `stored_start` is 1-based and names where the *first line of `block`* was
/// stored — the caller has already stepped it past any leading lines the block
/// dropped ([`ExpectedBlock::lead_offset`]). The stored position is checked
/// first, so a block that legitimately appears twice — a repeated idiom, a
/// duplicated `match` arm — is *not* ambiguous as long as it is still where it
/// was left.
pub(super) fn locate_block(lines: &[String], stored_start: usize, block: &[String]) -> BlockMatch {
    if block.is_empty() {
        return BlockMatch::AsStored;
    }
    // The file's significant lines, paired with the 1-based line they came from.
    let significant: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty())
        .collect();
    if significant.len() < block.len() {
        return BlockMatch::NotFound;
    }
    let matches_at = |offset: usize| {
        significant[offset..offset + block.len()]
            .iter()
            .zip(block)
            .all(|((_, have), want)| *have == want.as_str())
    };
    let mut found: Option<(usize, usize)> = None;
    let mut count = 0usize;
    for offset in 0..=(significant.len() - block.len()) {
        if !matches_at(offset) {
            continue;
        }
        let start = significant[offset].0;
        let end = significant[offset + block.len() - 1].0;
        if start == stored_start {
            return BlockMatch::AsStored;
        }
        count += 1;
        if count == 1 {
            found = Some((start, end));
        }
    }
    match (count, found) {
        (0, _) => BlockMatch::NotFound,
        (1, Some((start, end))) => BlockMatch::Moved { start, end },
        _ => BlockMatch::Ambiguous,
    }
}

/// What became of one stored row's anchor, given the file as it stands now.
///
/// `None` means "as stored, as far as can be told" — which covers both the
/// happy path and every row there is no evidence to judge: the project anchor,
/// a row whose selection was never captured, and a file that is there but
/// unreadable. Silence is the right answer for those; the alternative is a
/// marker that means "we didn't look", which is worse than no marker at all.
///
/// A *diff*-sourced selection is only ever reported lost, never re-anchored.
/// Its stored range is built from `new_line.or(old_line)`
/// ([`anchor_for_cursor`]), so a range that opens on a removed line is already
/// numbered off the base side of the diff — precise enough to point a reader at,
/// but not a baseline to measure movement against. "This code is no longer in
/// the file" is a claim that survives that; "it moved to line 61" is not.
pub(super) fn check_anchor_drift(
    qa: &LearningQa,
    target: &AnchorTarget,
) -> Option<LearningAnchorDrift> {
    if qa.file_path.is_none() || qa.anchor == LearningAnchor::Project {
        return None;
    }
    let lines = match target {
        AnchorTarget::Gone => {
            return Some(LearningAnchorDrift::Lost(LearningAnchorLoss::FileGone));
        }
        AnchorTarget::Unreadable => return None,
        AnchorTarget::Lines(lines) => lines,
    };
    // A whole-file anchor moves with its file: as long as the file is there,
    // the anchor is exactly as good as it ever was.
    let stored_start = match qa.anchor {
        LearningAnchor::File => return None,
        LearningAnchor::Hunk { .. } => 0,
        LearningAnchor::Lines { start, .. } => start,
        LearningAnchor::Project => return None,
    };
    let block = expected_block(&qa.selection_text, qa.selection_is_diff);
    // The evidence starts where the stored range starts *plus* whatever the
    // block dropped off the front, so a selection opening on a blank line is
    // still found where it was left. A hunk anchor has no stored line at all
    // (0 above, which no 1-based line can equal); shifting that sentinel would
    // turn it into a line number by accident.
    let evidence_start = match stored_start {
        0 => 0,
        start => start.saturating_add(block.lead_offset),
    };
    match locate_block(lines, evidence_start, &block.lines) {
        BlockMatch::AsStored => None,
        BlockMatch::NotFound => Some(LearningAnchorDrift::Lost(LearningAnchorLoss::NotFound)),
        BlockMatch::Ambiguous => Some(LearningAnchorDrift::Lost(LearningAnchorLoss::Ambiguous)),
        BlockMatch::Moved { start, end } => {
            if qa.selection_is_diff {
                // Found, so the code is still there — but see the note above on
                // why a diff-sourced range is not measured against.
                None
            } else {
                Some(LearningAnchorDrift::Reanchored { start, end })
            }
        }
    }
}

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
}

impl App {
    /// Move the Q&A history cursor.
    pub fn learning_select_qa(&mut self, delta: isize) {
        if let AppMode::Learning(state) = &mut self.mode {
            let len = state.qa.len();
            if len == 0 {
                return;
            }
            let moved_to = (state.selected_qa as isize + delta).clamp(0, len as isize - 1) as usize;
            state.select_qa(moved_to);
        }
    }
}

impl App {
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
                // Enter on a folder opens or closes it, so someone who never
                // finds `l`/`h` can still browse the tree with the one key
                // that already meant "open this".
                let on_dir = matches!(
                    &self.mode,
                    AppMode::Learning(state)
                        if matches!(state.selected_entry(), Some(LearningListEntry::Dir { .. }))
                );
                if on_dir {
                    self.learning_toggle_dir();
                } else if on_header {
                    self.learning_toggle_start_here();
                } else {
                    // Moving the cursor already loaded this file, so Enter on
                    // it is only a focus change. Reloading would re-read the
                    // file from disk and log the same failure twice, which is
                    // how this was caught. A file that *failed* still reloads:
                    // Enter is the only retry there is.
                    let already_loaded = matches!(
                        &self.mode,
                        AppMode::Learning(state)
                            if state.content_error.is_none()
                                && state.content_path.as_deref()
                                    == state.selected_entry().and_then(|e| e.path())
                    );
                    if !already_loaded {
                        self.learning_load_selected_content();
                    }
                    if let AppMode::Learning(state) = &mut self.mode {
                        state.focus = LearningFocus::Content;
                    }
                }
            }
            LearningFocus::Content => {}
            LearningFocus::Qa => self.learning_open_answer(),
        }
    }
}

impl App {
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
}

impl App {
    pub fn learning_close_answer(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.answer_open = false;
            state.answer_scroll = 0;
            state.answer_rendered_lines.clear();
            state.answer_rendered_width = 0;
        }
    }
}

impl App {
    pub fn learning_answer_scroll(&mut self, delta: isize) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.answer_scroll = (state.answer_scroll as isize + delta).max(0) as usize;
        }
    }
}

impl App {
    pub fn learning_answer_scroll_to_top(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.answer_scroll = 0;
        }
    }
}

impl App {
    /// Jump past the end; `draw_markdown_document` clamps to the real bottom
    /// once it knows how tall the rendered document is.
    pub fn learning_answer_scroll_to_bottom(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.answer_scroll = state.answer_rendered_lines.len().max(usize::MAX / 2);
        }
    }
}
