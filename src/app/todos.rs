//! Native TODOs overlay: open/close, navigation, and editing across the three
//! scopes a feature can file work under.
//!
//! The overlay shows up to three panes — the feature's own **worktree** list,
//! its **project** list, and the machine-wide **global** list. Project and
//! global visibility are independent, while a worktree pane is always shown.
//! Each pane keeps its own items, cursor, scroll, and scratchpad, so moving
//! focus or hiding a scope never disturbs the pane being left.
//!
//! Edits mutate the in-memory [`TodoPane`] and, when a DB is present, persist
//! the change. The in-memory panes are the source of truth for the overlay (so
//! it works without a DB, e.g. in tests).

use anyhow::Result;
use uuid::Uuid;

use crate::app::{
    App, AppMode, Selection, StartIntent, TodoDeleteDisposition, TodoImplementChoice,
    TodoImplementChoiceState, TodoLaunchAction, TodoLaunchStep, TodoPane, TodoPaneKind,
    TodoPlanDestination, TodoPlanOrigin, TodoScopeMoveState, TodoSpawnTargetState, TodoViewState,
    TodosHostReassignState,
};
use crate::db::todos::{Todo, TodoPriority, TodoScope, TodoStatus, TodoWorkState};

/// The selected TODO and the overlay context needed to act on it, gathered in
/// one read so callers do not re-borrow `self.mode` field by field.
pub(crate) struct SelectedTodoContext {
    pub todo: Todo,
    pub pi: usize,
    /// The feature the TODOs *session* lives under, used when the list's own
    /// host feature can no longer be resolved.
    pub fallback_fi: usize,
    pub host_feature_id: Option<String>,
    pub list_id: Option<String>,
    /// Which pane the item came from — the scope decides whether a spawn can
    /// pick its own feature or has to ask for one.
    pub pane_kind: TodoPaneKind,
    /// The list-level scratchpad note (`todo_lists.carry_over`).
    pub scratchpad: Option<String>,
}

/// What [`App::next_todo_index`] settled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NextTodo {
    /// Nothing has been started for it yet: put an agent on it without asking.
    Ready(usize),
    /// The best remaining candidate already links a session or a planned
    /// feature. Returned for the caller to ask about, never acted on silently.
    Started(usize),
}

/// One scope's list as "implement next" sees it.
pub(crate) struct ImplementNextList {
    pub kind: TodoPaneKind,
    /// The feature whose agent and mode a spawn from this list would inherit,
    /// when the scope names one. `None` for the global list, and unused for
    /// project scope, which asks the user regardless.
    pub host: Option<(usize, usize)>,
    pub todos: Vec<Todo>,
}

/// The lists "implement next" scans and the indices it acts under, gathered
/// from whichever surface invoked it.
pub(crate) struct ImplementNextCtx {
    pub pi: usize,
    /// The feature the TODOs *session* lives under, used when a list's own
    /// host feature can no longer be resolved.
    pub fallback_fi: usize,
    pub lists: Vec<ImplementNextList>,
}

impl ImplementNextCtx {
    /// Every list's items, in the order ties between scopes resolve.
    fn slices(&self) -> Vec<&[Todo]> {
        self.lists.iter().map(|l| l.todos.as_slice()).collect()
    }
}

impl App {
    // ----- scopes ---------------------------------------------------------

    /// Normalize a workdir into the key a worktree list is filed under.
    ///
    /// Trailing separators are the one difference that shows up in practice —
    /// a path typed with a slash and the same path without it are the same
    /// checkout — and letting them through would silently hand the user a
    /// second, empty list for a worktree they already have one for.
    pub(crate) fn todo_workdir_key(workdir: &std::path::Path) -> String {
        let raw = workdir.to_string_lossy();
        let trimmed = raw.trim_end_matches(std::path::MAIN_SEPARATOR);
        if trimmed.is_empty() {
            raw.to_string()
        } else {
            trimmed.to_string()
        }
    }

    /// The worktree scope for `(pi, fi)`, or `None` when that feature sits on
    /// the repo root and so has no checkout of its own to scope a list to.
    pub(crate) fn worktree_todo_scope(&self, pi: usize, fi: usize) -> Option<TodoScope> {
        let project = self.store.projects.get(pi)?;
        let feature = project.features.get(fi)?;
        feature.is_worktree.then(|| TodoScope::Worktree {
            project_id: project.id.clone(),
            workdir: Self::todo_workdir_key(&feature.workdir),
        })
    }

    /// The scope a write from `(pi, fi)` lands in when no pane says otherwise:
    /// the feature's worktree list, or the project's list at the repo root.
    ///
    /// Quick capture and the session-create path share this rule, so a note
    /// taken while working in a checkout goes to that checkout's list either
    /// way.
    pub(crate) fn default_todo_scope(&self, pi: usize, fi: usize) -> Option<TodoScope> {
        self.worktree_todo_scope(pi, fi).or_else(|| {
            self.store.projects.get(pi).map(|p| TodoScope::Project {
                project_id: p.id.clone(),
            })
        })
    }

    /// How a scope is named to the user: the scope's label plus what it is a
    /// list *for*.
    pub(crate) fn todo_scope_label(&self, scope: &TodoScope) -> String {
        match scope {
            TodoScope::Worktree { workdir, .. } => {
                let name = self
                    .store
                    .projects
                    .iter()
                    .flat_map(|p| p.features.iter())
                    .find(|f| Self::todo_workdir_key(&f.workdir) == *workdir)
                    .map(|f| f.name.clone())
                    .unwrap_or_else(|| {
                        std::path::Path::new(workdir)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| workdir.clone())
                    });
                format!("Worktree · {name}")
            }
            TodoScope::Project { project_id } => {
                let name = self
                    .store
                    .projects
                    .iter()
                    .find(|p| p.id == *project_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "project".to_string());
                format!("Project · {name}")
            }
            TodoScope::Global => "Global".to_string(),
        }
    }

    /// Whether a TODO scope is actionable in the current AMF process.
    /// Worktree TODOs are always visible; the two broader scopes share their
    /// visibility across every overlay opened during this run.
    pub(crate) fn todo_scope_visible(&self, scope: &TodoScope) -> bool {
        match scope {
            TodoScope::Worktree { .. } => true,
            TodoScope::Project { .. } => self.todo_project_visible,
            TodoScope::Global => self.todo_global_visible,
        }
    }

    /// Toggle one of the optional TODO scopes and return its new visibility.
    /// A worktree scope cannot be hidden, so toggling it is a no-op that
    /// reports `true`.
    pub(crate) fn toggle_todo_scope_visibility(&mut self, scope: &TodoScope) -> bool {
        match scope {
            TodoScope::Worktree { .. } => true,
            TodoScope::Project { .. } => {
                self.todo_project_visible = !self.todo_project_visible;
                self.todo_project_visible
            }
            TodoScope::Global => {
                self.todo_global_visible = !self.todo_global_visible;
                self.todo_global_visible
            }
        }
    }

    pub(crate) fn set_todo_scope_visibility(&mut self, scope: &TodoScope, visible: bool) {
        match scope {
            TodoScope::Worktree { .. } => {}
            TodoScope::Project { .. } => self.todo_project_visible = visible,
            TodoScope::Global => self.todo_global_visible = visible,
        }
    }

    // ----- open / close ---------------------------------------------------

    /// Open the native TODOs overlay for the TODOs session at `(pi, fi)`.
    ///
    /// The panes are resolved from the host feature: a worktree pane when the
    /// feature has a checkout of its own, then the project pane, then the
    /// global one. Lists are *loaded*, never created — an untouched scope
    /// leaves no row behind, and the first write creates what it needs.
    pub fn open_todos_view(&mut self, pi: usize, fi: usize) -> Result<()> {
        let (project_id, project_name, feature_name) = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi).map(|f| (p, f)))
        {
            Some((project, feature)) => (
                project.id.clone(),
                project.name.clone(),
                feature.name.clone(),
            ),
            None => return Ok(()),
        };

        let mut scopes: Vec<(TodoPaneKind, TodoScope, String)> = Vec::new();
        if let Some(scope) = self.worktree_todo_scope(pi, fi) {
            scopes.push((TodoPaneKind::Worktree, scope, feature_name.clone()));
        }
        scopes.push((
            TodoPaneKind::Project,
            TodoScope::Project {
                project_id: project_id.clone(),
            },
            project_name.clone(),
        ));
        scopes.push((TodoPaneKind::Global, TodoScope::Global, String::new()));

        let panes: Vec<TodoPane> = scopes
            .into_iter()
            .map(|(kind, scope, title)| {
                let (list, todos) = self.load_todos_for_scope(&scope);
                TodoPane {
                    kind,
                    scope,
                    title,
                    list,
                    todos,
                    selected: 0,
                    scroll_offset: 0,
                }
            })
            .collect();
        let focus = panes
            .iter()
            .position(|pane| self.todo_scope_visible(&pane.scope));

        self.mode = AppMode::Todos(TodoViewState {
            pi,
            fi,
            project_name,
            feature_name,
            panes,
            focus,
            editor: None,
            pending_delete: false,
            launch: None,
            scope_move: None,
        });
        Ok(())
    }

    /// Load `(list, todos)` for a scope from the DB, or `(None, empty)` when
    /// no DB is available or the scope has no list yet.
    fn load_todos_for_scope(
        &mut self,
        scope: &TodoScope,
    ) -> (
        Option<crate::db::todos::TodoList>,
        Vec<crate::db::todos::Todo>,
    ) {
        let Some(db) = &self.db else {
            return (None, Vec::new());
        };
        let list = match db.todo_list(scope) {
            Ok(list) => list,
            Err(e) => {
                let msg = format!("failed to load todo list: {e}");
                self.log_error("todos", msg);
                None
            }
        };
        let todos = match &list {
            Some(list) => self
                .db
                .as_ref()
                .and_then(|db| db.todos(&list.id).ok())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        (list, todos)
    }

    /// Close the overlay, returning to the dashboard with the TODOs session
    /// selected.
    pub fn close_todos_view(&mut self) {
        if let AppMode::Todos(state) = &self.mode {
            let (pi, fi) = (state.pi, state.fi);
            self.selection = self.todos_session_selection(pi, fi);
        }
        self.mode = AppMode::Normal;
        self.resume_paused_plan_interview();
    }

    /// Resolve a `Selection` for the TODOs session under `(pi, fi)`, falling
    /// back to the feature (then project) if it can't be found.
    fn todos_session_selection(&self, pi: usize, fi: usize) -> Selection {
        let si = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .and_then(|f| {
                f.sessions
                    .iter()
                    .position(|s| s.kind == crate::project::SessionKind::Todos)
            });
        match si {
            Some(si) => Selection::Session(pi, fi, si),
            None => Selection::Feature(pi, fi),
        }
    }

    // ----- panes ----------------------------------------------------------

    fn todos_pane(&self) -> Option<&TodoPane> {
        match &self.mode {
            AppMode::Todos(state) => state.focused(),
            _ => None,
        }
    }

    fn todos_pane_mut(&mut self) -> Option<&mut TodoPane> {
        match &mut self.mode {
            AppMode::Todos(state) => state.focused_mut(),
            _ => None,
        }
    }

    pub fn todos_select_next(&mut self) {
        if let Some(pane) = self.todos_pane_mut() {
            let len = pane.todos.len();
            if len > 0 {
                pane.selected = (pane.selected + 1) % len;
            }
        }
    }

    pub fn todos_select_prev(&mut self) {
        if let Some(pane) = self.todos_pane_mut() {
            let len = pane.todos.len();
            if len > 0 {
                pane.selected = if pane.selected == 0 {
                    len - 1
                } else {
                    pane.selected - 1
                };
            }
        }
    }

    /// `Tab` / `Shift+Tab`: move focus between visible actionable panes.
    pub fn todos_cycle_focus(&mut self, delta: isize) {
        let (project_visible, global_visible) =
            (self.todo_project_visible, self.todo_global_visible);
        let AppMode::Todos(state) = &mut self.mode else {
            return;
        };
        let visible = state.visible_pane_indices(project_visible, global_visible);
        if visible.is_empty() {
            self.push_toast_info("No TODO list is visible — press p or g to show one");
            return;
        }
        let current = state
            .focus
            .and_then(|focus| visible.iter().position(|index| *index == focus));
        let next = match current {
            Some(current) => (current as isize + delta).rem_euclid(visible.len() as isize) as usize,
            None if delta < 0 => visible.len() - 1,
            None => 0,
        };
        state.focus = Some(visible[next]);
    }

    fn todos_toggle_pane_visibility(&mut self, kind: TodoPaneKind) {
        let (pane_index, scope) = match &self.mode {
            AppMode::Todos(state) => match state
                .panes
                .iter()
                .enumerate()
                .find(|(_, pane)| pane.kind == kind)
            {
                Some((index, pane)) => (index, pane.scope.clone()),
                None => return,
            },
            _ => return,
        };

        let now_visible = self.toggle_todo_scope_visibility(&scope);
        let (project_visible, global_visible) =
            (self.todo_project_visible, self.todo_global_visible);
        let AppMode::Todos(state) = &mut self.mode else {
            return;
        };

        if now_visible {
            if state.focus.is_none() {
                state.focus = Some(pane_index);
            }
            return;
        }
        if state.focus != Some(pane_index) {
            return;
        }
        if state.panes.is_empty() {
            state.focus = None;
            return;
        }

        // Advance from the pane being hidden in the established ordering,
        // wrapping once. This naturally yields `None` when both optional
        // scopes are hidden and this feature has no worktree pane.
        state.focus = (1..=state.panes.len())
            .map(|offset| (pane_index + offset) % state.panes.len())
            .find(|index| {
                TodoViewState::pane_is_visible(
                    &state.panes[*index],
                    project_visible,
                    global_visible,
                )
            });
    }

    pub fn todos_toggle_project_visibility(&mut self) {
        self.todos_toggle_pane_visibility(TodoPaneKind::Project);
    }

    pub fn todos_toggle_global_visibility(&mut self) {
        self.todos_toggle_pane_visibility(TodoPaneKind::Global);
    }

    // ----- quick-capture from a session view ----------------------------

    /// Open the one-line TODO quick-capture over the current session view. The
    /// typed title is appended to the session feature's own list on commit —
    /// its worktree list, or the project's at the repo root — and the overlay
    /// names that list so the target is never a guess. No-op unless a session
    /// view is active.
    pub fn open_todo_quick_capture(&mut self) {
        let view = match &self.mode {
            AppMode::Viewing(view) => view.clone(),
            _ => return,
        };
        let project_name = view.project_name.clone();
        let list_label = self
            .viewing_indices(&view)
            .and_then(|(pi, fi)| self.default_todo_scope(pi, fi))
            .map(|scope| self.todo_scope_label(&scope))
            .unwrap_or_else(|| "this project's list".to_string());
        self.mode = AppMode::TodoQuickCapture(crate::app::TodoQuickCaptureState {
            view,
            project_name,
            list_label,
            input: String::new(),
        });
    }

    /// Cancel quick-capture, returning to the session view unchanged.
    pub fn cancel_todo_quick_capture(&mut self) {
        if let AppMode::TodoQuickCapture(state) = &self.mode {
            self.mode = AppMode::Viewing(state.view.clone());
        }
    }

    /// Append the typed title to the session feature's list, then return to
    /// the session view. An empty title is a no-op cancel. If the feature has
    /// no TODOs session yet, one is created (with its list) before the item is
    /// appended.
    pub fn commit_todo_quick_capture(&mut self) -> Result<()> {
        let (view, title) = match &self.mode {
            AppMode::TodoQuickCapture(state) => {
                (state.view.clone(), state.input.trim().to_string())
            }
            _ => return Ok(()),
        };

        if title.is_empty() {
            self.mode = AppMode::Viewing(view);
            return Ok(());
        }

        match self.viewing_indices(&view) {
            Some((pi, fi)) => match self.quick_capture_todo(pi, fi, &title) {
                Ok(()) => {
                    let shown = Self::truncate_title(&title, 40);
                    self.push_toast_success(format!("Added TODO: {shown}"));
                }
                Err(e) => {
                    self.log_error("todos", format!("quick capture failed: {e}"));
                    self.push_toast_error(format!("Failed to add TODO: {e}"));
                }
            },
            None => self.push_toast_warning("Couldn't resolve the project for this TODO"),
        }

        self.mode = AppMode::Viewing(view);
        Ok(())
    }

    /// Resolve the `(project, feature)` indices a session view belongs to by its
    /// project/feature names.
    fn viewing_indices(&self, view: &crate::app::ViewState) -> Option<(usize, usize)> {
        let pi = self
            .store
            .projects
            .iter()
            .position(|p| p.name == view.project_name)?;
        let fi = self.store.projects[pi]
            .features
            .iter()
            .position(|f| f.name == view.feature_name)?;
        Some((pi, fi))
    }

    /// Ensure the feature has a TODOs session (creating one when it doesn't),
    /// then append `title` to the scope that feature writes to by default.
    fn quick_capture_todo(&mut self, pi: usize, fi: usize, title: &str) -> Result<()> {
        // No-op create when the feature already has a TODOs session; otherwise
        // this adds one (and its list).
        self.add_todos_session_for_picker(pi, fi, None)?;

        let Some(scope) = self.default_todo_scope(pi, fi) else {
            anyhow::bail!("project not found");
        };
        let feature_id = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.id.clone());

        // Persist only when a DB is present; without one (tests) the session was
        // still created in memory, matching the overlay's DB-optional behavior.
        if let Some(db) = &self.db {
            let list = db.load_or_create_todo_list(&scope, feature_id.as_deref())?;
            db.add_todo(&list.id, title, None, TodoPriority::Med)?;
        }
        Ok(())
    }

    // ----- inline editing -----------------------------------------------

    /// Begin an inline edit, seeding the editor with `initial` text.
    fn todos_begin_edit(&mut self, target: crate::app::TodoEditTarget, initial: String) {
        use crate::app::TodoEditor;
        use crate::editor::TextEditor;
        if let AppMode::Todos(state) = &mut self.mode {
            state.editor = Some(TodoEditor {
                target,
                editor: TextEditor::new(initial),
            });
        }
    }

    /// Start adding a new TODO (empty title editor) in the focused pane.
    pub fn todos_begin_add(&mut self) {
        if self.todos_pane().is_some() {
            self.todos_begin_edit(crate::app::TodoEditTarget::New, String::new());
        }
    }

    /// Start editing the selected TODO's title.
    pub fn todos_begin_edit_title(&mut self) {
        let initial = self
            .todos_pane()
            .and_then(|p| p.selected_todo())
            .map(|t| t.title.clone());
        if let Some(initial) = initial {
            self.todos_begin_edit(crate::app::TodoEditTarget::Title, initial);
        }
    }

    /// Start editing the selected TODO's notes/detail body.
    pub fn todos_begin_edit_notes(&mut self) {
        let initial = self
            .todos_pane()
            .and_then(|p| p.selected_todo())
            .map(|t| t.body.clone().unwrap_or_default());
        if let Some(initial) = initial {
            self.todos_begin_edit(crate::app::TodoEditTarget::Notes, initial);
        }
    }

    /// Start editing the focused list's free-form scratchpad note.
    pub fn todos_begin_edit_scratchpad(&mut self) {
        let Some(pane) = self.todos_pane() else {
            return;
        };
        let initial = pane
            .list
            .as_ref()
            .and_then(|l| l.carry_over.clone())
            .unwrap_or_default();
        self.todos_begin_edit(crate::app::TodoEditTarget::Scratchpad, initial);
    }

    /// Cancel the active inline edit, discarding changes.
    pub fn todos_cancel_edit(&mut self) {
        if let AppMode::Todos(state) = &mut self.mode {
            state.editor = None;
        }
    }

    /// Commit the active inline edit, persisting it and refreshing the view.
    pub fn todos_commit_edit(&mut self) -> Result<()> {
        use crate::app::TodoEditTarget;
        let (target, text) = match &self.mode {
            AppMode::Todos(state) => match &state.editor {
                Some(ed) => (ed.target.clone(), ed.editor.text().to_string()),
                None => return Ok(()),
            },
            _ => return Ok(()),
        };

        match target {
            TodoEditTarget::New => {
                let title = text.trim();
                if !title.is_empty() {
                    self.todos_add(title.to_string())?;
                }
            }
            TodoEditTarget::Title => {
                let title = text.trim();
                if !title.is_empty() {
                    self.todos_update_selected(|t| t.title = title.to_string())?;
                }
            }
            TodoEditTarget::Notes => {
                let body = text.trim();
                let body = if body.is_empty() {
                    None
                } else {
                    Some(body.to_string())
                };
                self.todos_update_selected(|t| t.body = body.clone())?;
            }
            TodoEditTarget::Scratchpad => {
                self.todos_set_scratchpad(text.trim().to_string())?;
            }
        }

        if let AppMode::Todos(state) = &mut self.mode {
            state.editor = None;
        }
        Ok(())
    }

    /// Resolve the list id for pane `pane_index`, creating the list on first
    /// write. With a DB this persists the list; without one it synthesizes an
    /// in-memory list so edits still work.
    ///
    /// A worktree or project list is hosted by the feature the overlay was
    /// opened under; the global list has no host feature to record.
    fn todos_ensure_list_id_for(&mut self, pane_index: usize) -> Option<String> {
        let (scope, existing, pi, fi) = match &self.mode {
            AppMode::Todos(state) => {
                let pane = state.panes.get(pane_index)?;
                (
                    pane.scope.clone(),
                    pane.list.as_ref().map(|l| l.id.clone()),
                    state.pi,
                    state.fi,
                )
            }
            _ => return None,
        };
        if let Some(id) = existing {
            return Some(id);
        }

        let feature_id = match scope {
            TodoScope::Global => None,
            _ => self
                .store
                .projects
                .get(pi)
                .and_then(|p| p.features.get(fi))
                .map(|f| f.id.clone()),
        };

        let list = match self.db.as_ref() {
            Some(db) => match db.load_or_create_todo_list(&scope, feature_id.as_deref()) {
                Ok(list) => list,
                Err(e) => {
                    self.log_error("todos", format!("failed to create todo list: {e}"));
                    return None;
                }
            },
            None => crate::db::todos::TodoList {
                id: Uuid::new_v4().to_string(),
                scope,
                feature_id,
                carry_over: None,
                created_at: String::new(),
                updated_at: String::new(),
            },
        };
        let id = list.id.clone();
        if let AppMode::Todos(state) = &mut self.mode
            && let Some(pane) = state.panes.get_mut(pane_index)
        {
            pane.list = Some(list);
        }
        Some(id)
    }

    /// The focused pane's list id, created on first write.
    fn todos_ensure_list_id(&mut self) -> Option<String> {
        let focus = match &self.mode {
            AppMode::Todos(state) => state.focus?,
            _ => return None,
        };
        self.todos_ensure_list_id_for(focus)
    }

    /// Append a new TODO with `title` to the focused pane, persisting and
    /// selecting it.
    fn todos_add(&mut self, title: String) -> Result<()> {
        let list_id = self.todos_ensure_list_id();

        // Persist via DB when available; otherwise build an in-memory item.
        let next_order = self
            .todos_pane()
            .and_then(|p| p.todos.iter().map(|t| t.sort_order).max())
            .map(|m| m + 1)
            .unwrap_or(0);

        let new_todo = match (&self.db, &list_id) {
            (Some(db), Some(list_id)) => db.add_todo(list_id, &title, None, TodoPriority::Med)?,
            _ => Todo {
                id: Uuid::new_v4().to_string(),
                list_id: list_id.unwrap_or_default(),
                title,
                body: None,
                priority: TodoPriority::Med,
                sort_order: next_order,
                work: TodoWorkState::default(),
                linked_feature_id: None,
                created_at: String::new(),
                updated_at: String::new(),
            },
        };

        let new_id = new_todo.id.clone();
        if let Some(pane) = self.todos_pane_mut() {
            pane.todos.push(new_todo);
            Self::resort_todos(&mut pane.todos);
            if let Some(pos) = pane.todos.iter().position(|t| t.id == new_id) {
                pane.selected = pos;
            }
        }
        Ok(())
    }

    /// Mutate the focused pane's selected TODO in place, persisting the change.
    fn todos_update_selected(&mut self, f: impl FnOnce(&mut Todo)) -> Result<()> {
        let updated = match self.todos_pane_mut() {
            Some(pane) => match pane.todos.get_mut(pane.selected) {
                Some(todo) => {
                    f(todo);
                    todo.clone()
                }
                None => return Ok(()),
            },
            None => return Ok(()),
        };
        if let Some(db) = &self.db {
            db.update_todo(&updated)?;
        }
        // Re-sort in case status changed, keeping the cursor on the same item.
        if let Some(pane) = self.todos_pane_mut() {
            Self::resort_todos(&mut pane.todos);
            if let Some(pos) = pane.todos.iter().position(|t| t.id == updated.id) {
                pane.selected = pos;
            }
        }
        Ok(())
    }

    /// Cycle the selected TODO through not started, in progress, and completed.
    pub fn todos_toggle_done(&mut self) -> Result<()> {
        self.todos_update_selected(|t| t.work.cycle_manually())
    }

    /// Compatibility entry point for the former separate in-progress toggle.
    /// It now follows the same single status cycle as every manual toggle.
    pub fn todos_toggle_in_progress(&mut self) -> Result<()> {
        self.todos_toggle_done()
    }

    /// Cycle the selected TODO's priority High → Med → Low → High.
    pub fn todos_cycle_priority(&mut self) -> Result<()> {
        self.todos_update_selected(|t| {
            t.priority = match t.priority {
                TodoPriority::High => TodoPriority::Med,
                TodoPriority::Med => TodoPriority::Low,
                TodoPriority::Low => TodoPriority::High,
            };
        })
    }

    /// Move the selected TODO up (`delta = -1`) or down (`delta = 1`) in the
    /// focused pane, persisting the new `sort_order` for that list.
    pub fn todos_reorder(&mut self, delta: isize) -> Result<()> {
        let ids: Vec<String> = match self.todos_pane_mut() {
            Some(pane) => {
                let len = pane.todos.len();
                if len < 2 {
                    return Ok(());
                }
                let cur = pane.selected;
                let target = cur as isize + delta;
                if target < 0 || target as usize >= len {
                    return Ok(());
                }
                let target = target as usize;
                pane.todos.swap(cur, target);
                // Renumber sort_order to the new display positions.
                for (i, todo) in pane.todos.iter_mut().enumerate() {
                    todo.sort_order = i as i64;
                }
                Self::resort_todos(&mut pane.todos);
                // Follow the moved item.
                let moved_id = pane.todos[target].id.clone();
                if let Some(pos) = pane.todos.iter().position(|t| t.id == moved_id) {
                    pane.selected = pos;
                }
                pane.todos.iter().map(|t| t.id.clone()).collect()
            }
            None => return Ok(()),
        };
        if let Some(db) = &self.db {
            db.reorder_todos(&ids)?;
        }
        Ok(())
    }

    /// Update the focused list's scratchpad note, persisting it. (Stored in
    /// the legacy `carry_over` column / `set_todo_carry_over` DB method.)
    fn todos_set_scratchpad(&mut self, note: String) -> Result<()> {
        let list_id = self.todos_ensure_list_id();
        let value = if note.is_empty() { None } else { Some(note) };
        if let (Some(db), Some(list_id)) = (&self.db, &list_id) {
            db.set_todo_carry_over(list_id, value.as_deref())?;
        }
        if let Some(pane) = self.todos_pane_mut()
            && let Some(list) = &mut pane.list
        {
            list.carry_over = value;
        }
        Ok(())
    }

    // ----- move / copy between scopes -------------------------------------

    /// `M` / `C`: choose another scope to re-file the selected TODO into.
    ///
    /// Every other visible pane is offered. Hidden scopes are not actionable
    /// until the user reveals them again.
    pub fn todos_begin_scope_move(&mut self, copy: bool) {
        let (project_visible, global_visible) =
            (self.todo_project_visible, self.todo_global_visible);
        let AppMode::Todos(state) = &self.mode else {
            return;
        };
        let Some(todo) = state.focused().and_then(|p| p.selected_todo()) else {
            self.push_toast_warning("No TODO selected");
            return;
        };
        let (todo_id, todo_title) = (todo.id.clone(), todo.title.clone());
        let Some(focus) = state.focus else {
            self.push_toast_warning("No visible TODO list is selected");
            return;
        };
        let targets: Vec<(String, usize)> = state
            .panes
            .iter()
            .enumerate()
            .filter(|(i, pane)| {
                *i != focus && TodoViewState::pane_is_visible(pane, project_visible, global_visible)
            })
            .map(|(i, pane)| {
                let label = if pane.title.is_empty() {
                    pane.kind.label().to_string()
                } else {
                    format!("{} · {}", pane.kind.label(), pane.title)
                };
                (label, i)
            })
            .collect();

        if targets.is_empty() {
            self.push_toast_info("There is no other list to move this TODO to");
            return;
        }

        if let AppMode::Todos(state) = &mut self.mode {
            state.scope_move = Some(TodoScopeMoveState {
                copy,
                todo_id,
                todo_title,
                targets,
                selected: 0,
            });
        }
    }

    pub fn todo_scope_move_cursor(&mut self, delta: isize) {
        if let AppMode::Todos(state) = &mut self.mode
            && let Some(step) = &mut state.scope_move
        {
            step.move_cursor(delta);
        }
    }

    pub fn cancel_todo_scope_move(&mut self) {
        if let AppMode::Todos(state) = &mut self.mode {
            state.scope_move = None;
        }
    }

    /// Apply the chosen move or copy.
    ///
    /// A **move** carries the item's links with it — it is the same work, and
    /// the session someone started for it is still that work in flight. A
    /// **copy** lands unstarted, so two panes never both claim one session.
    pub fn confirm_todo_scope_move(&mut self) -> Result<()> {
        let (copy, todo_id, todo_title, target_index, source_index) = match &self.mode {
            AppMode::Todos(state) => match &state.scope_move {
                Some(step) => match step.targets.get(step.selected) {
                    Some((_, target)) => (
                        step.copy,
                        step.todo_id.clone(),
                        step.todo_title.clone(),
                        *target,
                        match state.focus {
                            Some(focus) => focus,
                            None => return Ok(()),
                        },
                    ),
                    None => return Ok(()),
                },
                None => return Ok(()),
            },
            _ => return Ok(()),
        };

        // Re-resolve the item: the list can have changed under the prompt.
        let Some(source_pos) = self.todos_position_in_pane(source_index, &todo_id) else {
            self.cancel_todo_scope_move();
            self.push_toast_warning("That TODO is no longer in the list");
            return Ok(());
        };

        let Some(target_list_id) = self.todos_ensure_list_id_for(target_index) else {
            self.cancel_todo_scope_move();
            self.push_toast_error("Couldn't open the destination list");
            return Ok(());
        };

        let target_label = match &self.mode {
            AppMode::Todos(state) => state
                .panes
                .get(target_index)
                .map(|p| p.kind.label().to_string())
                .unwrap_or_default(),
            _ => String::new(),
        };

        let shown = Self::truncate_title(&todo_title, 40);
        if copy {
            let copied = match &self.db {
                Some(db) => db.copy_todo(&todo_id, &target_list_id)?,
                None => None,
            };
            let copied = copied.or_else(|| {
                // No DB (tests): build the same unstarted duplicate in memory.
                let source = self.todos_in_pane(source_index)?.get(source_pos)?;
                Some(Todo {
                    id: Uuid::new_v4().to_string(),
                    list_id: target_list_id.clone(),
                    title: source.title.clone(),
                    body: source.body.clone(),
                    priority: source.priority,
                    sort_order: 0,
                    work: TodoWorkState {
                        status: if source.work.status == TodoStatus::Completed {
                            TodoStatus::Completed
                        } else {
                            TodoStatus::NotStarted
                        },
                        agent_session_id: None,
                    },
                    linked_feature_id: None,
                    created_at: String::new(),
                    updated_at: String::new(),
                })
            });
            if let (Some(copied), AppMode::Todos(state)) = (copied, &mut self.mode)
                && let Some(pane) = state.panes.get_mut(target_index)
            {
                let mut copied = copied;
                copied.sort_order = pane
                    .todos
                    .iter()
                    .map(|t| t.sort_order)
                    .max()
                    .map(|m| m + 1)
                    .unwrap_or(0);
                pane.todos.push(copied);
                Self::resort_todos(&mut pane.todos);
            }
            self.push_toast_success(format!("Copied to the {target_label} list: {shown}"));
        } else {
            if let Some(db) = &self.db {
                db.move_todo(&todo_id, &target_list_id)?;
            }
            if let AppMode::Todos(state) = &mut self.mode {
                let mut moved = state.panes[source_index].todos.remove(source_pos);
                let pane = &mut state.panes[source_index];
                if pane.selected >= pane.todos.len() {
                    pane.selected = pane.todos.len().saturating_sub(1);
                }
                if let Some(target) = state.panes.get_mut(target_index) {
                    moved.list_id = target_list_id.clone();
                    moved.sort_order = target
                        .todos
                        .iter()
                        .map(|t| t.sort_order)
                        .max()
                        .map(|m| m + 1)
                        .unwrap_or(0);
                    target.todos.push(moved);
                    Self::resort_todos(&mut target.todos);
                }
            }
            self.push_toast_success(format!("Moved to the {target_label} list: {shown}"));
        }

        self.cancel_todo_scope_move();
        Ok(())
    }

    fn todos_in_pane(&self, pane_index: usize) -> Option<&[Todo]> {
        match &self.mode {
            AppMode::Todos(state) => state.panes.get(pane_index).map(|p| p.todos.as_slice()),
            _ => None,
        }
    }

    fn todos_position_in_pane(&self, pane_index: usize, todo_id: &str) -> Option<usize> {
        self.todos_in_pane(pane_index)?
            .iter()
            .position(|t| t.id == todo_id)
    }

    // ----- spawn agent ---------------------------------------------------

    /// `Enter` on the selected TODO.
    ///
    /// Resolves what the key means before offering a choice, because a TODO
    /// that already has somewhere to go should go there rather than ask again:
    ///
    /// 1. A linked feature (a previous plan-mode run created one) — jump to it.
    /// 2. A live linked session — jump to it, as this key always has.
    /// 3. Otherwise — open the chooser.
    ///
    /// The feature link wins when a TODO carries both. A feature is the larger
    /// destination: the session link points at one agent, while the feature
    /// link points at a whole checkout created for this item, and that is
    /// where the work moved to.
    ///
    /// A link whose target is gone is dropped rather than reported as a dead
    /// end, so the next press offers the chooser instead of failing again.
    pub fn todos_launch_selected(&mut self) -> Result<()> {
        let Some(ctx) = self.selected_todo_context() else {
            self.push_toast_warning("No TODO selected");
            return Ok(());
        };
        let (todo, pi, list_id) = (ctx.todo, ctx.pi, ctx.list_id);
        let fi =
            self.resolve_todo_host_feature(pi, ctx.host_feature_id.as_deref(), ctx.fallback_fi);

        // 1. A feature created for this TODO by an earlier plan run.
        if let Some(feature_id) = todo.linked_feature_id.as_deref() {
            match self.feature_indices_by_id(feature_id) {
                Some((fpi, ffi)) => return self.jump_to_linked_feature(fpi, ffi),
                None => {
                    self.clear_todo_linked_feature(&todo.id)?;
                    self.push_toast_warning(
                        "The feature planned for this TODO no longer exists; the link was cleared",
                    );
                }
            }
        }

        // 2. A still-live session spawned for this TODO. Searched across every
        //    feature, not just this list's host: a project- or global-scoped
        //    TODO's agent lives wherever the user chose to put it.
        if let Some(session_id) = todo.work.agent_session_id.as_deref()
            && let Some((spi, sfi, si)) = self.session_indices_by_id(session_id)
        {
            self.selection = Selection::Session(spi, sfi, si);
            return self.enter_view();
        }

        // 3. Nothing to return to: ask what this key should do.
        let host_feature_id = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.id.clone())
            .unwrap_or_default();
        self.open_todo_launch_choice(TodoPlanOrigin {
            todo_id: todo.id.clone(),
            list_id: list_id.unwrap_or_default(),
            todo_title: todo.title.clone(),
            host_feature_id,
        });
        Ok(())
    }

    /// The selected TODO plus everything acting on it needs from the overlay.
    pub(crate) fn selected_todo_context(&self) -> Option<SelectedTodoContext> {
        match &self.mode {
            AppMode::Todos(state) => {
                let pane = state.focused()?;
                Some(SelectedTodoContext {
                    todo: pane.selected_todo().cloned()?,
                    pi: state.pi,
                    fallback_fi: state.fi,
                    host_feature_id: pane.list.as_ref().and_then(|l| l.feature_id.clone()),
                    list_id: pane.list.as_ref().map(|l| l.id.clone()),
                    pane_kind: pane.kind,
                    scratchpad: pane.list.as_ref().and_then(|l| l.carry_over.clone()),
                })
            }
            _ => None,
        }
    }

    /// Locate a feature anywhere in the store by id.
    pub(crate) fn feature_indices_by_id(&self, feature_id: &str) -> Option<(usize, usize)> {
        self.store.projects.iter().enumerate().find_map(|(pi, p)| {
            p.features
                .iter()
                .position(|f| f.id == feature_id)
                .map(|fi| (pi, fi))
        })
    }

    /// Locate a session anywhere in the store by id.
    ///
    /// A TODO's session used to be guaranteed to live in the list's host
    /// feature. It no longer is: a project- or global-scoped TODO's agent
    /// runs in whichever feature the user picked, and a moved TODO carries its
    /// link across scopes. Searching everywhere is what keeps "is this session
    /// still alive?" a question about the session rather than about which list
    /// happens to hold the row.
    pub(crate) fn session_indices_by_id(&self, session_id: &str) -> Option<(usize, usize, usize)> {
        self.store.projects.iter().enumerate().find_map(|(pi, p)| {
            p.features.iter().enumerate().find_map(|(fi, f)| {
                f.sessions
                    .iter()
                    .position(|s| s.id == session_id)
                    .map(|si| (pi, fi, si))
            })
        })
    }

    /// Select the feature a TODO was planned into and open it.
    ///
    /// Its first agent session is preferred over the feature row, since the
    /// point of the jump is to get back to the agent working the plan; a
    /// feature with no agent session selects the feature itself.
    fn jump_to_linked_feature(&mut self, pi: usize, fi: usize) -> Result<()> {
        let agent_si = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .and_then(|f| f.sessions.iter().position(|s| s.kind.is_agent_harness()));
        self.selection = match agent_si {
            Some(si) => Selection::Session(pi, fi, si),
            None => Selection::Feature(pi, fi),
        };
        self.enter_view()
    }

    /// Drop a TODO's dead feature link, in memory and (with a DB) on disk.
    ///
    /// The DB write is targeted by id rather than an `update_todo` of the
    /// overlay row, for the same reason [`Self::todos_mark_in_progress`] is: this
    /// also runs from the dashboard's "implement next", where no overlay is
    /// open and there is no in-memory row to write back.
    fn clear_todo_linked_feature(&mut self, todo_id: &str) -> Result<()> {
        if let AppMode::Todos(state) = &mut self.mode {
            for pane in state.panes.iter_mut() {
                if let Some(todo) = pane.todos.iter_mut().find(|t| t.id == todo_id) {
                    todo.linked_feature_id = None;
                }
            }
        }
        if let Some(db) = &self.db {
            db.clear_todo_linked_feature_for_todo(todo_id)?;
        }
        Ok(())
    }

    // ----- launch chooser -------------------------------------------------

    fn open_todo_launch_choice(&mut self, origin: TodoPlanOrigin) {
        if let AppMode::Todos(state) = &mut self.mode {
            state.launch = Some(TodoLaunchStep::Choice {
                origin,
                selected: 0,
            });
        }
    }

    pub fn todo_launch_step_move(&mut self, delta: isize) {
        if let AppMode::Todos(state) = &mut self.mode
            && let Some(step) = &mut state.launch
        {
            step.move_cursor(delta);
        }
    }

    /// `Esc`: unwind one step — destination back to the chooser, chooser back
    /// to the list.
    pub fn cancel_todo_launch_step(&mut self) {
        if let AppMode::Todos(state) = &mut self.mode {
            state.launch = match state.launch.take() {
                Some(TodoLaunchStep::Destination { origin, .. }) => Some(TodoLaunchStep::Choice {
                    origin,
                    // Return the cursor to the option that got here.
                    selected: 1,
                }),
                _ => None,
            };
        }
    }

    pub fn confirm_todo_launch_step(&mut self) -> Result<()> {
        let step = match &self.mode {
            AppMode::Todos(state) => state.launch.clone(),
            _ => None,
        };
        let Some(step) = step else { return Ok(()) };

        match (&step, step.action(), step.destination()) {
            (_, Some(TodoLaunchAction::SpawnSession), _) => {
                self.close_todo_launch_step();
                self.todos_spawn_agent()
            }
            (_, Some(TodoLaunchAction::PlanMode), _) => {
                self.todos_mark_in_progress(&step.origin().todo_id, None)?;
                self.open_todo_plan_destination(step.origin().clone());
                Ok(())
            }
            (_, _, Some(TodoPlanDestination::HostFeature)) => {
                let origin = step.origin().clone();
                self.close_todo_launch_step();
                self.start_todo_plan_in_host_feature(origin)
            }
            (
                TodoLaunchStep::Destination {
                    can_create_worktree,
                    ..
                },
                _,
                Some(TodoPlanDestination::NewFeature),
            ) => {
                if !*can_create_worktree {
                    self.push_toast_warning(
                        "This project has no git repository, so a new worktree cannot be created",
                    );
                    return Ok(());
                }
                let origin = step.origin().clone();
                self.close_todo_launch_step();
                self.start_todo_plan_in_new_feature(origin)
            }
            _ => Ok(()),
        }
    }

    fn close_todo_launch_step(&mut self) {
        if let AppMode::Todos(state) = &mut self.mode {
            state.launch = None;
        }
    }

    fn open_todo_plan_destination(&mut self, origin: TodoPlanOrigin) {
        let (host_feature_name, can_create_worktree) = match &self.mode {
            AppMode::Todos(state) => {
                let host_feature_id = state
                    .focused()
                    .and_then(|p| p.list.as_ref())
                    .and_then(|l| l.feature_id.clone());
                let fi =
                    self.resolve_todo_host_feature(state.pi, host_feature_id.as_deref(), state.fi);
                let project = self.store.projects.get(state.pi);
                (
                    project
                        .and_then(|p| p.features.get(fi))
                        .map(|f| f.name.clone())
                        .unwrap_or_else(|| state.feature_name.clone()),
                    project.is_some_and(|p| p.is_git),
                )
            }
            _ => return,
        };
        if let AppMode::Todos(state) = &mut self.mode {
            state.launch = Some(TodoLaunchStep::Destination {
                origin,
                host_feature_name,
                can_create_worktree,
                selected: 0,
            });
        }
    }

    /// The chooser's *Start an agent on this TODO*: spawn in the feature the
    /// scope names, or ask for one when the scope names none.
    pub fn todos_spawn_agent(&mut self) -> Result<()> {
        let Some(ctx) = self.selected_todo_context() else {
            self.push_toast_warning("No TODO selected");
            return Ok(());
        };
        let fi =
            self.resolve_todo_host_feature(ctx.pi, ctx.host_feature_id.as_deref(), ctx.fallback_fi);
        self.launch_todo_in_scope(ctx.pane_kind, ctx.pi, fi, ctx.todo, false)
    }

    /// Put an agent on `todo`, deciding *where* from the scope it lives in.
    ///
    /// A worktree TODO belongs to exactly one checkout, so it spawns there
    /// without asking. A project or global TODO belongs to no single checkout
    /// — there is nothing to infer — so the user names the feature, and that
    /// feature supplies the agent and mode as always.
    fn launch_todo_in_scope(
        &mut self,
        kind: TodoPaneKind,
        pi: usize,
        fi: usize,
        todo: Todo,
        force_new: bool,
    ) -> Result<()> {
        match kind {
            TodoPaneKind::Worktree => self.spawn_todo_agent(pi, fi, &todo, force_new),
            TodoPaneKind::Project | TodoPaneKind::Global => {
                self.open_todo_spawn_target(kind, todo, force_new, Some((pi, fi)));
                Ok(())
            }
        }
    }

    /// Open the "which feature should work this?" picker for a project- or
    /// global-scoped TODO.
    ///
    /// A project TODO lists that project's features; a global one lists every
    /// project's, since a global TODO carries no project of its own. The
    /// cursor starts on `default` — the feature the overlay was opened under —
    /// so the common case is one keypress.
    fn open_todo_spawn_target(
        &mut self,
        kind: TodoPaneKind,
        todo: Todo,
        force_new: bool,
        default: Option<(usize, usize)>,
    ) {
        let project_filter = match kind {
            TodoPaneKind::Global => None,
            _ => default.map(|(pi, _)| pi),
        };
        let candidates: Vec<(String, usize, usize)> = self
            .store
            .projects
            .iter()
            .enumerate()
            .filter(|(pi, _)| project_filter.is_none_or(|want| *pi == want))
            .flat_map(|(pi, project)| {
                project
                    .features
                    .iter()
                    .enumerate()
                    .map(move |(fi, feature)| {
                        (format!("{} / {}", project.name, feature.name), pi, fi)
                    })
            })
            .collect();

        if candidates.is_empty() {
            self.push_toast_warning("There is no feature to put an agent on this TODO in");
            return;
        }

        let selected = default
            .and_then(|(dpi, dfi)| {
                candidates
                    .iter()
                    .position(|(_, pi, fi)| *pi == dpi && *fi == dfi)
            })
            .unwrap_or(0);

        let origin = std::mem::replace(&mut self.mode, AppMode::Normal);
        self.mode = AppMode::TodoSpawnTarget(Box::new(TodoSpawnTargetState {
            origin: Box::new(origin),
            todo,
            pane_kind: kind,
            candidates,
            selected,
            force_new,
        }));
    }

    pub fn todo_spawn_target_move(&mut self, delta: isize) {
        if let AppMode::TodoSpawnTarget(state) = &mut self.mode {
            state.move_cursor(delta);
        }
    }

    /// `Esc`: change nothing and go back to where the key was pressed.
    pub fn cancel_todo_spawn_target(&mut self) {
        if let AppMode::TodoSpawnTarget(_) = &self.mode {
            let AppMode::TodoSpawnTarget(state) =
                std::mem::replace(&mut self.mode, AppMode::Normal)
            else {
                return;
            };
            self.mode = *state.origin;
        }
    }

    pub fn confirm_todo_spawn_target(&mut self) -> Result<()> {
        let AppMode::TodoSpawnTarget(_) = &self.mode else {
            return Ok(());
        };
        let AppMode::TodoSpawnTarget(state) = std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            return Ok(());
        };
        let state = *state;
        let target = state.selection();
        // Act from the mode the key was pressed in: the overlay's in-memory
        // panes are its source of truth, so the spawn has to see them rather
        // than the empty Normal mode this prompt was holding.
        self.mode = *state.origin;

        let Some((pi, fi)) = target else {
            self.push_toast_warning("No feature selected");
            return Ok(());
        };
        self.spawn_todo_agent(pi, fi, &state.todo, state.force_new)
    }

    /// Launch an agent on `todo` in feature `(pi, fi)` and seed the composer
    /// with it, editable and unsent.
    ///
    /// A TODO that already links a live session reuses it — jumped to and added
    /// onto — rather than accumulating a second agent for the same item, unless
    /// `force_new` says the user asked for exactly that. The reused session is
    /// looked up store-wide rather than inside `(pi, fi)`: a project- or
    /// global-scoped TODO's agent may be running in a different feature
    /// entirely. Either way the TODO is marked started before the view
    /// changes, so the list can show it as underway and "implement next" scans
    /// past it.
    ///
    /// Takes the TODO by value rather than reading `self.mode`, because the
    /// dashboard's "implement next" spawns with no overlay open.
    pub(crate) fn spawn_todo_agent(
        &mut self,
        pi: usize,
        fi: usize,
        todo: &Todo,
        force_new: bool,
    ) -> Result<()> {
        let prompt = Self::todo_spawn_prompt(todo);

        if todo.work.status == TodoStatus::InProgress {
            self.push_toast_warning(
                "This TODO is already in progress; another agent was not launched",
            );
            return Ok(());
        }
        if todo.work.status == TodoStatus::Completed {
            self.push_toast_warning("Mark this TODO not started before launching an agent");
            return Ok(());
        }

        let existing = if force_new {
            None
        } else {
            todo.work
                .agent_session_id
                .as_deref()
                .and_then(|sid| self.session_indices_by_id(sid))
        };

        if !self.todos_reserve_launch(todo)? {
            self.push_toast_warning(
                "This TODO is already in progress; another agent was not launched",
            );
            return Ok(());
        }

        let (pi, fi, si) = match existing {
            Some(found) => found,
            None => {
                let agent = self
                    .store
                    .projects
                    .get(pi)
                    .and_then(|p| p.features.get(fi))
                    .map(|f| f.agent.clone())
                    .unwrap_or_default();
                let label = Self::todo_session_label(&todo.title);
                // The link back to the TODO is recorded from inside the
                // Todos overlay, which the confirmation dialog would replace,
                // so this start warns instead of parking.
                match self.create_agent_session_labeled(
                    pi,
                    fi,
                    &label,
                    Some(agent),
                    StartIntent::Warn("the agent for this TODO"),
                ) {
                    Ok(si) => (pi, fi, si),
                    Err(e) => {
                        self.todos_rollback_launch_best_effort(&todo.id);
                        self.push_toast_error(format!("Failed to launch agent: {e}"));
                        return Ok(());
                    }
                }
            }
        };

        let Some(session_id) = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .and_then(|f| f.sessions.get(si))
            .map(|s| s.id.clone())
        else {
            self.todos_rollback_launch_best_effort(&todo.id);
            self.push_toast_error("The session for this TODO vanished as it was created");
            return Ok(());
        };
        if let Err(e) = self.todos_mark_in_progress(&todo.id, Some(&session_id)) {
            self.todos_rollback_launch_best_effort(&todo.id);
            return Err(e);
        }

        // Switch into the session view and seed the composer (editable). The
        // seed is not submitted, so the user reviews it before sending.
        self.selection = Selection::Session(pi, fi, si);
        if let Err(e) = self
            .enter_view_without_auto_compose()
            .and_then(|_| self.open_compose_seeded(prompt))
        {
            self.todos_rollback_launch_best_effort(&todo.id);
            return Err(e);
        }
        Ok(())
    }

    /// Resolve the feature index that hosts the list within project `pi`: the
    /// feature whose id matches `host_feature_id`, else `fallback_fi` (the
    /// feature the TODOs session itself lives under).
    pub(crate) fn resolve_todo_host_feature(
        &self,
        pi: usize,
        host_feature_id: Option<&str>,
        fallback_fi: usize,
    ) -> usize {
        host_feature_id
            .and_then(|fid| {
                self.store
                    .projects
                    .get(pi)
                    .and_then(|p| p.features.iter().position(|f| f.id == fid))
            })
            .unwrap_or(fallback_fi)
    }

    /// Build the composer seed for a spawned TODO: a fixed lead-in, the title,
    /// then the notes body when present.
    pub(crate) fn todo_spawn_prompt(todo: &Todo) -> String {
        let mut prompt = format!(
            "Please address this TODO item for this feature:\n\n{}",
            todo.title.trim()
        );
        if let Some(body) = todo
            .body
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
        {
            prompt.push_str("\n\n");
            prompt.push_str(body);
        }
        prompt
    }

    /// Compose the feature brief a TODO-originated plan interview opens on.
    ///
    /// Everything the TODO row actually carries goes in: its title as the
    /// heading, its notes, the list-level scratchpad, and a **provenance**
    /// paragraph naming work already started for it. The brief is a starting
    /// point, not a submission — the interview's `Brief` phase opens it in an
    /// editor, so a wrong guess here costs the user an edit, not an answer.
    ///
    /// Provenance is deliberately a statement *that* work happened, not a
    /// transcript of it. AMF keeps no per-TODO history: transcripts are scoped
    /// to a workdir (and in practice to one harness), and a tmux capture holds
    /// only whatever is still on screen. Either would put text in the brief
    /// that does not reliably describe this TODO, which is worse than a line
    /// the reader can trust.
    pub(crate) fn compose_plan_brief(
        todo: &Todo,
        scratchpad: Option<&str>,
        provenance: &[String],
    ) -> String {
        let mut brief = format!("## {}\n", todo.title.trim());

        if let Some(body) = todo
            .body
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
        {
            brief.push('\n');
            brief.push_str(body);
            brief.push('\n');
        }

        // The scratchpad belongs to the list, not the item, so it is labelled
        // as the shared note it is rather than folded in as if the user had
        // written it about this TODO.
        if let Some(note) = scratchpad.map(str::trim).filter(|n| !n.is_empty()) {
            brief.push_str("\n## List scratchpad\n\n");
            brief.push_str(note);
            brief.push('\n');
        }

        if !provenance.is_empty() {
            brief.push_str("\n## Prior work\n\n");
            for line in provenance {
                brief.push_str(line);
                brief.push('\n');
            }
        }

        brief
    }

    /// Provenance lines for `todo`: what AMF has already started for it.
    ///
    /// Each link is reported only when its target still exists, and says so
    /// when it does not, because a brief that names a session the user cannot
    /// find is worse than one that says the session is gone. The session is
    /// looked for store-wide, since a project- or global-scoped TODO's agent
    /// need not be in the feature the brief is being written under.
    pub(crate) fn todo_provenance(&self, _pi: usize, _fi: usize, todo: &Todo) -> Vec<String> {
        let mut lines = Vec::new();

        if let Some(session_id) = todo.work.agent_session_id.as_deref() {
            let found = self
                .session_indices_by_id(session_id)
                .and_then(|(pi, fi, si)| {
                    let feature = self.store.projects.get(pi)?.features.get(fi)?;
                    let label = feature.sessions.get(si)?.label.clone();
                    Some((label, feature.clone()))
                });
            match found {
                Some((label, feature)) => {
                    let live = feature.status != crate::project::ProjectStatus::Stopped
                        && self.tmux.session_exists(&feature.tmux_session);
                    lines.push(format!(
                        "- An agent session \"{label}\" was already started for this item in \
                         feature \"{}\" and is {}.",
                        feature.name,
                        if live { "still running" } else { "not running" }
                    ));
                }
                None => lines.push(
                    "- An agent session was started for this item earlier, but it no longer \
                     exists."
                        .to_string(),
                ),
            }
        }

        if let Some(feature_id) = todo.linked_feature_id.as_deref() {
            let found = self
                .store
                .projects
                .iter()
                .flat_map(|project| project.features.iter())
                .find(|feature| feature.id == feature_id);
            match found {
                Some(feature) => lines.push(format!(
                    "- A feature \"{}\" was already created for this item from a previous plan \
                     run, on branch \"{}\".",
                    feature.name, feature.branch
                )),
                None => lines.push(
                    "- A feature was created for this item from a previous plan run, but it has \
                     since been deleted."
                        .to_string(),
                ),
            }
        }

        lines
    }

    /// Truncate `title` to at most `max` characters, appending an ellipsis when
    /// it was shortened."""
    fn truncate_title(title: &str, max: usize) -> String {
        if title.chars().count() > max {
            let truncated: String = title.chars().take(max).collect();
            format!("{}…", truncated.trim_end())
        } else {
            title.to_string()
        }
    }

    /// A short session label derived from a TODO title (truncated).
    pub(crate) fn todo_session_label(title: &str) -> String {
        const MAX: usize = 24;
        let title = title.trim();
        if title.chars().count() > MAX {
            let truncated: String = title.chars().take(MAX).collect();
            format!("TODO: {}…", truncated.trim_end())
        } else {
            format!("TODO: {title}")
        }
    }

    /// Reserve a TODO before any agent/session side effect is attempted.
    pub(crate) fn todos_reserve_launch(&mut self, todo: &Todo) -> Result<bool> {
        let mut work = todo.work.clone();
        if !work.reserve_launch() {
            return Ok(false);
        }
        if let Some(db) = &self.db {
            db.set_todo_work_state(&todo.id, &work)?;
        }
        if let AppMode::Todos(state) = &mut self.mode {
            for pane in &mut state.panes {
                if let Some(loaded) = pane.todos.iter_mut().find(|item| item.id == todo.id) {
                    loaded.work = work.clone();
                }
            }
        }
        Ok(true)
    }

    /// Prepare the agent launch that follows an accepted TODO plan.
    ///
    /// Plan mode normally marked the item in progress when the workflow began,
    /// so that state is permission to continue rather than a duplicate-launch
    /// conflict. A not-started item can still occur when accepting an older
    /// draft or after an external status edit; reserve it through the ordinary
    /// launch path (this always succeeds, since `todo`'s status was just
    /// checked here and nothing re-reads it in between) and tell the caller
    /// that a startup failure should undo that new reservation. Completed work
    /// remains blocked.
    pub(crate) fn todos_prepare_planned_launch(&mut self, todo: &Todo) -> Result<Option<bool>> {
        match todo.work.status {
            TodoStatus::InProgress => Ok(Some(false)),
            TodoStatus::NotStarted => {
                self.todos_reserve_launch(todo)?;
                Ok(Some(true))
            }
            TodoStatus::Completed => Ok(None),
        }
    }

    /// Restore the pre-launch state after agent creation or prompt setup fails.
    pub(crate) fn todos_rollback_launch(&mut self, todo_id: &str) -> Result<()> {
        let mut work = TodoWorkState::default();
        work.rollback_launch();
        if let Some(db) = &self.db {
            db.set_todo_work_state(todo_id, &work)?;
        }
        if let AppMode::Todos(state) = &mut self.mode {
            for pane in &mut state.panes {
                if let Some(todo) = pane.todos.iter_mut().find(|item| item.id == todo_id) {
                    todo.work = work.clone();
                }
            }
        }
        Ok(())
    }

    /// [`Self::todos_rollback_launch`] for a failure-handling arm that already
    /// has a more specific error or toast to report.
    ///
    /// `?` on the rollback itself would let a *second* failure (the rollback's
    /// DB write) replace that message with an opaque, unrelated one, so this
    /// logs a rollback failure instead of propagating it — the caller's
    /// original error is always what reaches the user.
    pub(crate) fn todos_rollback_launch_best_effort(&mut self, todo_id: &str) {
        if let Err(e) = self.todos_rollback_launch(todo_id) {
            self.log_warn(
                "todos",
                format!("failed to roll back reservation for TODO {todo_id}: {e}"),
            );
        }
    }

    /// Mark a TODO in progress, optionally associating the session produced for
    /// it, in memory (across every loaded pane) and on disk.
    ///
    /// Passing no session preserves an existing association, which is the
    /// status-only transition used when plan mode begins. Passing a session is
    /// the successful agent-launch transition. Repeating either transition on
    /// an already in-progress TODO is intentionally harmless.
    ///
    /// The targeted work-state write leaves scope, list identity, ordering,
    /// title, notes, priority, and feature linkage untouched. It also works
    /// from the dashboard's "implement next", where no overlay is open.
    pub(crate) fn todos_mark_in_progress(
        &mut self,
        todo_id: &str,
        session_id: Option<&str>,
    ) -> Result<()> {
        let mut persisted = self
            .find_todo_by_id(todo_id)
            .map(|todo| todo.work)
            .unwrap_or_default();
        persisted.status = TodoStatus::InProgress;
        if let Some(session_id) = session_id {
            persisted.agent_session_id = Some(session_id.to_string());
        }

        // Persist first so a failed write does not make the visible overlay
        // claim the action succeeded when its source of truth did not change.
        if let Some(db) = &self.db {
            db.set_todo_work_state(todo_id, &persisted)?;
        }
        if let AppMode::Todos(state) = &mut self.mode {
            for pane in state.panes.iter_mut() {
                if let Some(todo) = pane.todos.iter_mut().find(|t| t.id == todo_id) {
                    todo.work = persisted.clone();
                }
            }
        }
        Ok(())
    }

    /// Drop any TODO's link to a feature that has just been deleted.
    ///
    /// Separate from [`Self::handle_todos_host_feature_deleted`], which is
    /// about the *list's* home: this is about individual rows that were planned
    /// into the deleted feature. The TODO survives — the work it describes
    /// outlived the branch — and the next `Enter` offers the chooser again rather
    /// than a jump that cannot land.
    pub(crate) fn clear_todo_links_to_deleted_feature(&mut self, feature_id: Option<&str>) {
        let Some(feature_id) = feature_id else { return };
        if let Some(db) = self.db.as_ref()
            && let Err(e) = db.clear_todo_linked_feature(feature_id)
        {
            self.log_warn(
                "todos",
                format!("failed to clear TODO links to deleted feature {feature_id}: {e}"),
            );
        }
        // The overlay is not open during a deletion, but keep any loaded rows
        // truthful rather than relying on that.
        if let AppMode::Todos(state) = &mut self.mode {
            for pane in state.panes.iter_mut() {
                for todo in pane
                    .todos
                    .iter_mut()
                    .filter(|t| t.linked_feature_id.as_deref() == Some(feature_id))
                {
                    todo.linked_feature_id = None;
                }
            }
        }
    }

    // ----- implement next ------------------------------------------------

    /// `I` on a TODOs session row on the dashboard: take the highest-priority
    /// unstarted TODO from the scopes that are currently visible and put an
    /// agent on it.
    ///
    /// Inert on anything but a TODOs session row — a feature with no list has
    /// no row to press it on, which is the whole gate.
    pub fn implement_next_todo_from_dashboard(&mut self) -> Result<()> {
        let Selection::Session(pi, fi, si) = self.selection else {
            return Ok(());
        };
        let is_todos_session = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .and_then(|f| f.sessions.get(si))
            .is_some_and(|s| s.kind == crate::project::SessionKind::Todos);
        if !is_todos_session {
            return Ok(());
        }
        let ctx = self.implement_next_ctx(pi, fi);
        self.implement_next(ctx, Vec::new())
    }

    /// `I` inside the TODOs overlay. Same scan as the dashboard's, over the
    /// panes already loaded, and deliberately distinct from `Enter`, which
    /// stays on the item under the cursor.
    pub fn implement_next_todo_in_overlay(&mut self) -> Result<()> {
        let AppMode::Todos(state) = &self.mode else {
            return Ok(());
        };
        let (pi, fi) = (state.pi, state.fi);
        let ctx = self.implement_next_ctx(pi, fi);
        self.implement_next(ctx, Vec::new())
    }

    /// The scopes a surface counts as visible for `(pi, fi)`, in the order
    /// ties between them resolve.
    ///
    /// The worktree scope is unconditional; project and global use the shared
    /// process-lifetime visibility flags.
    pub(crate) fn visible_todo_scopes(
        &self,
        pi: usize,
        fi: usize,
    ) -> Vec<(TodoPaneKind, TodoScope)> {
        let mut scopes = Vec::new();
        if let Some(scope) = self.worktree_todo_scope(pi, fi) {
            scopes.push((TodoPaneKind::Worktree, scope));
        }
        if self.todo_project_visible
            && let Some(project) = self.store.projects.get(pi)
        {
            scopes.push((
                TodoPaneKind::Project,
                TodoScope::Project {
                    project_id: project.id.clone(),
                },
            ));
        }
        if self.todo_global_visible {
            scopes.push((TodoPaneKind::Global, TodoScope::Global));
        }
        scopes
    }

    /// Gather the lists to scan: the overlay's visible panes when it is open
    /// (they are its source of truth, and may hold edits a DB-less run never
    /// persisted), otherwise the visible scopes read from the DB.
    fn implement_next_ctx(&mut self, pi: usize, fallback_fi: usize) -> ImplementNextCtx {
        if let AppMode::Todos(state) = &self.mode {
            let (pi, fallback_fi) = (state.pi, state.fi);
            let visible =
                state.visible_pane_indices(self.todo_project_visible, self.todo_global_visible);
            let lists = state
                .panes
                .iter()
                .enumerate()
                .filter(|(index, _)| visible.contains(index))
                .map(|(_, pane)| pane)
                .map(|pane| ImplementNextList {
                    kind: pane.kind,
                    host: match pane.kind {
                        TodoPaneKind::Global => None,
                        _ => Some((
                            pi,
                            self.resolve_todo_host_feature(
                                pi,
                                pane.list.as_ref().and_then(|l| l.feature_id.as_deref()),
                                fallback_fi,
                            ),
                        )),
                    },
                    todos: pane.todos.clone(),
                })
                .collect();
            return ImplementNextCtx {
                pi,
                fallback_fi,
                lists,
            };
        }

        let scopes = self.visible_todo_scopes(pi, fallback_fi);
        let mut lists = Vec::new();
        for (kind, scope) in scopes {
            let (list, todos) = self.load_todos_for_scope(&scope);
            let host = match kind {
                TodoPaneKind::Global => None,
                _ => Some((
                    pi,
                    self.resolve_todo_host_feature(
                        pi,
                        list.as_ref().and_then(|l| l.feature_id.as_deref()),
                        fallback_fi,
                    ),
                )),
            };
            lists.push(ImplementNextList { kind, host, todos });
        }
        ImplementNextCtx {
            pi,
            fallback_fi,
            lists,
        }
    }

    /// Run the scan and act on what it finds.
    fn implement_next(&mut self, mut ctx: ImplementNextCtx, skipped: Vec<String>) -> Result<()> {
        if ctx.lists.is_empty() {
            self.push_toast_info("No TODO list is visible — press p or g to show one");
            return Ok(());
        }
        for list in ctx.lists.iter_mut() {
            self.todos_reconcile_dead_sessions(&mut list.todos)?;
        }

        let found = {
            let slices = ctx.slices();
            Self::next_todo_across(&slices, &skipped)
        };

        match found {
            None => {
                let slices = ctx.slices();
                self.push_toast_info(Self::no_next_todo_message_across(&slices, &skipped));
                Ok(())
            }
            Some((li, NextTodo::Ready(i))) => {
                let list = &ctx.lists[li];
                let todo = list.todos[i].clone();
                let (kind, host) = (list.kind, list.host);
                let (pi, fi) = host.unwrap_or((ctx.pi, ctx.fallback_fi));
                self.launch_todo_in_scope(kind, pi, fi, todo, false)
            }
            Some((li, NextTodo::Started(i))) => {
                let list = &ctx.lists[li];
                let todo = &list.todos[i];
                let (todo_id, todo_title) = (todo.id.clone(), todo.title.clone());
                let kind = list.kind;
                let host_feature_id = list.host.and_then(|(pi, fi)| {
                    self.store
                        .projects
                        .get(pi)
                        .and_then(|p| p.features.get(fi))
                        .map(|f| f.id.clone())
                });
                let origin = std::mem::replace(&mut self.mode, AppMode::Normal);
                self.mode = AppMode::TodoImplementChoice(Box::new(TodoImplementChoiceState {
                    origin: Box::new(origin),
                    pi: ctx.pi,
                    fallback_fi: ctx.fallback_fi,
                    host_feature_id,
                    pane_kind: kind,
                    todo_id,
                    todo_title,
                    skipped_ids: skipped,
                    selected: 0,
                }));
                Ok(())
            }
        }
    }

    /// The TODO "implement next" should act on within a single list, or `None`
    /// if there is none. The one-list form of [`Self::next_todo_across`],
    /// which is what the scan itself calls; this exists so the per-list rules
    /// can be pinned down on their own.
    #[cfg(test)]
    pub(crate) fn next_todo_index(todos: &[Todo], skipped_ids: &[String]) -> Option<NextTodo> {
        Self::next_todo_across(&[todos], skipped_ids).map(|(_, next)| next)
    }

    /// The TODO "implement next" should act on across the visible scopes, as
    /// `(list index, choice)`.
    ///
    /// Priority first (High, then Med, then Low). Within a priority the lists
    /// are considered in the order they are given — worktree, then project,
    /// then global — and within a list the order it is already in, because the
    /// sort is stable: a manual ordering the user arranged is what breaks ties
    /// inside a list, and scope is what breaks them between lists. Completed,
    /// in-progress, and explicitly skipped items are passed over entirely.
    ///
    /// A TODO that already links a session or a planned feature is not
    /// *chosen*, it is held in reserve: an unstarted item anywhere in the scan
    /// wins over it, and it is only returned — as [`NextTodo::Started`], for
    /// the caller to ask about — when nothing unstarted remains. That is what
    /// reconciles "skip TODOs that already have a session" with there being a
    /// prompt for exactly that case.
    pub(crate) fn next_todo_across(
        lists: &[&[Todo]],
        skipped_ids: &[String],
    ) -> Option<(usize, NextTodo)> {
        let mut order: Vec<(usize, usize)> = lists
            .iter()
            .enumerate()
            .flat_map(|(li, todos)| (0..todos.len()).map(move |i| (li, i)))
            .collect();
        order.sort_by_key(|&(li, i)| lists[li][i].priority.rank());

        let mut started: Option<(usize, usize)> = None;
        for (li, i) in order {
            let todo = &lists[li][i];
            if !todo.is_eligible_for_automatic_spawn()
                || skipped_ids.iter().any(|id| id == &todo.id)
            {
                continue;
            }
            if todo.work.agent_session_id.is_none() && todo.linked_feature_id.is_none() {
                return Some((li, NextTodo::Ready(i)));
            }
            if started.is_none() {
                started = Some((li, i));
            }
        }
        started.map(|(li, i)| (li, NextTodo::Started(i)))
    }

    /// Why the scan came back empty, said in the terms the user can act on.
    /// The one-list form of [`Self::no_next_todo_message_across`].
    #[cfg(test)]
    pub(crate) fn no_next_todo_message(todos: &[Todo], skipped_ids: &[String]) -> String {
        Self::no_next_todo_message_across(&[todos], skipped_ids)
    }

    /// Why the scan came back empty, across every list it looked at.
    ///
    /// A blanket "nothing to do" would be wrong in the case that matters:
    /// items are there, they are just all underway, and the fix is to finish
    /// or un-flag one rather than to add more.
    pub(crate) fn no_next_todo_message_across(lists: &[&[Todo]], skipped_ids: &[String]) -> String {
        let open = lists
            .iter()
            .flat_map(|todos| todos.iter())
            .filter(|t| !t.work.status.is_completed())
            .count();
        if open == 0 {
            "No TODOs left to implement".to_string()
        } else if !skipped_ids.is_empty() {
            "No other TODOs left to implement".to_string()
        } else {
            "All remaining TODOs are already in progress".to_string()
        }
    }

    /// Drop session links whose session is gone without changing status.
    ///
    /// A session counts as gone only when it exists in **no** feature:
    /// a project- or global-scoped TODO's agent may be running somewhere other
    /// than the list's host. The in-progress status intentionally remains: a
    /// missing agent does not make work unstarted again.
    fn todos_reconcile_dead_sessions(&mut self, todos: &mut [Todo]) -> Result<()> {
        let dead: Vec<String> = todos
            .iter()
            .filter(|t| {
                t.work
                    .agent_session_id
                    .as_deref()
                    .is_some_and(|sid| self.session_indices_by_id(sid).is_none())
            })
            .map(|t| t.id.clone())
            .collect();
        if dead.is_empty() {
            return Ok(());
        }

        for todo in todos.iter_mut().filter(|t| dead.contains(&t.id)) {
            todo.work.clear_missing_session();
        }
        if let AppMode::Todos(state) = &mut self.mode {
            for pane in state.panes.iter_mut() {
                for todo in pane.todos.iter_mut().filter(|t| dead.contains(&t.id)) {
                    todo.work.clear_missing_session();
                }
            }
        }
        if let Some(db) = &self.db {
            for id in &dead {
                db.clear_todo_agent_session(id)?;
            }
        }
        Ok(())
    }

    /// Reconcile every persisted TODO association against the sessions in the
    /// project store. This runs from the ordinary status sync, including the
    /// startup sync, so stale links heal without opening the TODO overlay.
    pub(crate) fn reconcile_todo_agent_associations(&mut self) -> Result<()> {
        let Some(db) = self.db.as_ref() else {
            return Ok(());
        };
        let live_ids: std::collections::HashSet<&str> = self
            .store
            .projects
            .iter()
            .flat_map(|project| project.features.iter())
            .flat_map(|feature| feature.sessions.iter())
            .map(|session| session.id.as_str())
            .collect();
        let stale: Vec<String> = db
            .todo_agent_session_associations()?
            .into_iter()
            .filter_map(|(todo_id, session_id)| {
                (!live_ids.contains(session_id.as_str())).then_some(todo_id)
            })
            .collect();

        for todo_id in &stale {
            db.clear_todo_agent_session(todo_id)?;
        }
        if let AppMode::Todos(state) = &mut self.mode {
            for pane in &mut state.panes {
                for todo in pane
                    .todos
                    .iter_mut()
                    .filter(|todo| stale.contains(&todo.id))
                {
                    todo.work.clear_missing_session();
                }
            }
        }
        Ok(())
    }

    /// Re-read a TODO by id from whichever list is authoritative right now.
    /// Used when acting on a prompt, so the list changing underneath it is
    /// noticed rather than acted on stale.
    ///
    /// Looked up by id alone, not a remembered `list_id`: the TODO may have
    /// been moved or copied to a different list since the caller last saw
    /// it, and a list-scoped lookup would silently miss it in that case.
    pub(crate) fn find_todo_by_id(&self, todo_id: &str) -> Option<Todo> {
        if let AppMode::Todos(state) = &self.mode {
            return state
                .panes
                .iter()
                .find_map(|pane| pane.todos.iter().find(|t| t.id == todo_id))
                .cloned();
        }
        self.db.as_ref()?.find_todo_by_id(todo_id).ok()?
    }

    // ----- already-started prompt -----------------------------------------

    pub fn todo_implement_choice_move(&mut self, delta: isize) {
        if let AppMode::TodoImplementChoice(state) = &mut self.mode {
            state.move_cursor(delta);
        }
    }

    /// `Esc`, and the *Cancel* option: change nothing and go back to where the
    /// key was pressed.
    pub fn cancel_todo_implement_choice(&mut self) {
        if let AppMode::TodoImplementChoice(_) = &self.mode {
            let AppMode::TodoImplementChoice(state) =
                std::mem::replace(&mut self.mode, AppMode::Normal)
            else {
                return;
            };
            self.mode = *state.origin;
        }
    }

    pub fn confirm_todo_implement_choice(&mut self) -> Result<()> {
        let AppMode::TodoImplementChoice(_) = &self.mode else {
            return Ok(());
        };
        let AppMode::TodoImplementChoice(state) =
            std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            return Ok(());
        };
        let choice = state.choice();
        let state = *state;
        // Every branch acts from the mode the key was pressed in: the overlay's
        // in-memory panes are its source of truth, so a spawn or a re-scan has
        // to see them rather than the empty Normal mode this prompt was
        // holding.
        self.mode = *state.origin;

        match choice {
            TodoImplementChoice::Cancel => Ok(()),
            TodoImplementChoice::SkipToNext => {
                let mut skipped = state.skipped_ids;
                skipped.push(state.todo_id);
                let ctx = self.implement_next_ctx(state.pi, state.fallback_fi);
                self.implement_next(ctx, skipped)
            }
            TodoImplementChoice::Jump | TodoImplementChoice::SpawnNew => {
                let Some(todo) = self.find_todo_by_id(&state.todo_id) else {
                    self.push_toast_warning("That TODO is no longer in the list");
                    return Ok(());
                };
                let fi = self.resolve_todo_host_feature(
                    state.pi,
                    state.host_feature_id.as_deref(),
                    state.fallback_fi,
                );
                if choice == TodoImplementChoice::SpawnNew {
                    return self.launch_todo_in_scope(state.pane_kind, state.pi, fi, todo, true);
                }
                self.jump_to_started_todo(&todo)
            }
        }
    }

    /// Go to whatever an earlier run created for this TODO.
    ///
    /// The feature link wins over the session link for the same reason
    /// [`Self::todos_launch_selected`] prefers it: a planned feature is a whole
    /// checkout made for this item, while the session link is one agent inside
    /// it.
    ///
    /// A link whose feature is gone is *cleared*, exactly as `g`/`Enter` clears
    /// it, and for a sharper reason here: the link is the only thing holding
    /// the TODO back from [`NextTodo::Ready`], so leaving it would make every
    /// future "implement next" offer the same item and every jump fail the same
    /// way. Dropping it lets the next scan pick the TODO up and start it.
    fn jump_to_started_todo(&mut self, todo: &Todo) -> Result<()> {
        let mut cleared_feature_link = false;
        if let Some(feature_id) = todo.linked_feature_id.as_deref() {
            match self.feature_indices_by_id(feature_id) {
                Some((fpi, ffi)) => return self.jump_to_linked_feature(fpi, ffi),
                None => {
                    self.clear_todo_linked_feature(&todo.id)?;
                    cleared_feature_link = true;
                    self.push_toast_warning(
                        "The feature planned for this TODO no longer exists; the link was cleared",
                    );
                }
            }
        }
        if let Some((pi, fi, si)) = todo
            .work
            .agent_session_id
            .as_deref()
            .and_then(|sid| self.session_indices_by_id(sid))
        {
            self.selection = Selection::Session(pi, fi, si);
            return self.enter_view();
        }
        if !cleared_feature_link {
            self.push_toast_warning("The work started for this TODO is gone");
        }
        Ok(())
    }

    // ----- delete --------------------------------------------------------

    /// Ask to delete the selected TODO (awaits y/n confirmation).
    pub fn todos_request_delete(&mut self) {
        let has_items = self.todos_pane().is_some_and(|pane| !pane.todos.is_empty());
        if has_items && let AppMode::Todos(state) = &mut self.mode {
            state.pending_delete = true;
        }
    }

    pub fn todos_cancel_delete(&mut self) {
        if let AppMode::Todos(state) = &mut self.mode {
            state.pending_delete = false;
        }
    }

    /// Delete the selected TODO. The linked session, if any, is left untouched.
    pub fn todos_confirm_delete(&mut self) -> Result<()> {
        if let AppMode::Todos(state) = &mut self.mode {
            state.pending_delete = false;
        }
        let removed_id = match self.todos_pane_mut() {
            Some(pane) => {
                if pane.todos.is_empty() {
                    return Ok(());
                }
                let id = pane.todos[pane.selected].id.clone();
                pane.todos.remove(pane.selected);
                if pane.selected >= pane.todos.len() {
                    pane.selected = pane.todos.len().saturating_sub(1);
                }
                id
            }
            None => return Ok(()),
        };
        if let Some(db) = &self.db {
            db.delete_todo(&removed_id)?;
        }
        Ok(())
    }

    /// Sort items into display order: open first, then by manual `sort_order`.
    fn resort_todos(todos: &mut [Todo]) {
        todos.sort_by(|a, b| {
            a.work
                .status
                .is_completed()
                .cmp(&b.work.status.is_completed())
                .then(a.sort_order.cmp(&b.sort_order))
        });
    }

    // ----- feature deletion ----------------------------------------------

    /// The disposition prompt a feature deletion has to answer first, if its
    /// worktree list still holds unfinished work.
    ///
    /// Returns `Some(state)` when the caller must stop and ask. Deleting a
    /// worktree is hard to reverse and takes its list with it, so the TODOs in
    /// it are not something to decide on the user's behalf. A list with
    /// nothing open in it — empty, or everything ticked off — is not worth a
    /// prompt: there is no work to lose.
    pub(crate) fn pending_todo_disposition(
        &self,
        project_name: &str,
        feature_name: &str,
    ) -> Option<crate::app::TodoDeleteDispositionState> {
        let db = self.db.as_ref()?;
        let project = self.store.find_project(project_name)?;
        let feature = project.features.iter().find(|f| f.name == feature_name)?;
        if !feature.is_worktree {
            return None;
        }
        let scope = TodoScope::Worktree {
            project_id: project.id.clone(),
            workdir: Self::todo_workdir_key(&feature.workdir),
        };
        let list = db.todo_list(&scope).ok()??;
        let unfinished = db
            .todos(&list.id)
            .ok()?
            .into_iter()
            .filter(|t| !t.work.status.is_completed())
            .count();
        if unfinished == 0 {
            return None;
        }
        Some(crate::app::TodoDeleteDispositionState {
            project_name: project_name.to_string(),
            feature_name: feature_name.to_string(),
            feature_id: feature.id.clone(),
            project_id: project.id.clone(),
            workdir: Self::todo_workdir_key(&feature.workdir),
            list_id: list.id,
            unfinished,
            selected: 0,
        })
    }

    pub fn todo_delete_disposition_move(&mut self, delta: isize) {
        if let AppMode::TodoDeleteDisposition(state) = &mut self.mode {
            state.move_cursor(delta);
        }
    }

    /// `Esc` is *Cancel*: nothing is deleted and the feature stays.
    pub fn cancel_todo_delete_disposition(&mut self) {
        if let AppMode::TodoDeleteDisposition(_) = &self.mode {
            self.mode = AppMode::Normal;
            self.message = Some("Deletion cancelled".to_string());
        }
    }

    /// Apply the chosen disposition, then hand back to the delete flow.
    ///
    /// *Cancel* stops here with the feature and its worktree intact. The other
    /// three settle the TODOs first and only then let the deletion run, so a
    /// failure to re-file them never happens after the worktree is already
    /// gone.
    pub fn confirm_todo_delete_disposition(&mut self) -> Result<()> {
        let AppMode::TodoDeleteDisposition(_) = &self.mode else {
            return Ok(());
        };
        let AppMode::TodoDeleteDisposition(state) =
            std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            return Ok(());
        };
        let choice = state.choice();

        if choice == TodoDeleteDisposition::Cancel {
            self.message = Some("Deletion cancelled".to_string());
            return Ok(());
        }

        self.apply_todo_disposition(&state, choice)?;
        self.mode = AppMode::DeletingFeature(state.project_name, state.feature_name);
        self.delete_feature()
    }

    /// Settle the worktree list's items, then drop the list.
    ///
    /// Split out from [`Self::confirm_todo_delete_disposition`] so the
    /// re-filing can be exercised on its own: the caller goes on to run a
    /// deletion that kills tmux sessions and removes a git worktree, which is
    /// not what this half is about.
    pub(crate) fn apply_todo_disposition(
        &mut self,
        state: &crate::app::TodoDeleteDispositionState,
        choice: TodoDeleteDisposition,
    ) -> Result<()> {
        let target_scope = match choice {
            TodoDeleteDisposition::MoveToProject => Some(TodoScope::Project {
                project_id: state.project_id.clone(),
            }),
            TodoDeleteDisposition::MoveToGlobal => Some(TodoScope::Global),
            _ => None,
        };

        // The destination may not exist yet — the project list is created
        // lazily, and so is the global one.
        //
        // The host must be a feature that *survives* this deletion. Hosting a
        // freshly-created project list on the feature being deleted would hand
        // it straight to `handle_todos_host_feature_deleted`, which either
        // deletes the list outright (no features left — losing the items the
        // user just chose to keep) or raises a re-home prompt for a list that
        // was created moments ago. With no survivor the list is created
        // hostless: `resolve_todo_host_feature` treats the host as a hint, and
        // an unhosted list is still found by scope once the project has a
        // feature again.
        let host_feature_id = self
            .store
            .find_project(&state.project_name)
            .and_then(|p| p.features.iter().find(|f| f.id != state.feature_id))
            .map(|f| f.id.clone());

        let mut moved: Option<(usize, &'static str)> = None;
        if let Some(db) = self.db.as_ref() {
            if let Some(scope) = target_scope {
                let host = match scope {
                    TodoScope::Global => None,
                    _ => host_feature_id.as_deref(),
                };
                let target = db.load_or_create_todo_list(&scope, host)?;
                let moving: Vec<String> = db
                    .todos(&state.list_id)?
                    .into_iter()
                    .filter(|t| !t.work.status.is_completed())
                    .map(|t| t.id)
                    .collect();
                for id in &moving {
                    db.move_todo(id, &target.id)?;
                }
                moved = Some((moving.len(), scope.as_db_str()));
            }
            // Whatever is left in the worktree list goes with the worktree:
            // on a move that is only the completed items, on a delete it is
            // everything.
            db.delete_worktree_todo_list(&state.project_id, &state.workdir)?;
        }
        if let Some((count, scope_name)) = moved {
            self.log_info(
                "todos",
                format!(
                    "moved {count} unfinished TODO(s) from the '{}' worktree list to the \
                     {scope_name} list",
                    state.feature_name
                ),
            );
        }
        Ok(())
    }

    /// Called after a feature is removed from a surviving project. If the
    /// deleted feature hosted the project's TODO list, decide the list's fate:
    ///
    /// - No features remain → silently delete the now-orphaned list.
    /// - Surviving features exist → open [`AppMode::TodosHostReassign`] so the
    ///   user re-homes the list onto another feature or deletes it.
    ///
    /// Only the *project*-scoped list is at stake: a worktree list belongs to
    /// a checkout rather than to a host feature, and it was already settled by
    /// the disposition prompt before the deletion ran.
    ///
    /// Returns `true` if it opened the re-home prompt (so the caller leaves the
    /// mode alone). A no-op without a DB, since todo lists only persist there.
    pub fn handle_todos_host_feature_deleted(
        &mut self,
        project_name: &str,
        deleted_feature_name: &str,
        deleted_feature_id: Option<&str>,
    ) -> bool {
        let Some(deleted_feature_id) = deleted_feature_id else {
            return false;
        };
        let Some(db) = self.db.as_ref() else {
            return false;
        };
        let Some(project) = self.store.find_project(project_name) else {
            return false;
        };
        let project_id = project.id.clone();
        let scope = TodoScope::Project {
            project_id: project_id.clone(),
        };
        let list = match db.todo_list(&scope) {
            Ok(Some(list)) => list,
            _ => return false,
        };
        if list.feature_id.as_deref() != Some(deleted_feature_id) {
            // The deleted feature did not host the list; nothing to do.
            return false;
        }

        // Surviving features (the deleted one is already gone from the store).
        let candidates: Vec<(String, String)> = project
            .features
            .iter()
            .map(|f| (f.name.clone(), f.id.clone()))
            .collect();

        if candidates.is_empty() {
            // Nothing left to host the list — drop it and its items.
            if let Err(e) = db.delete_todo_list(&list.id) {
                self.log_error(
                    "todos",
                    format!("failed to delete orphaned todo list {}: {e}", list.id),
                );
            } else {
                self.log_info(
                    "todos",
                    format!("deleted orphaned todo list for project '{project_name}'"),
                );
            }
            return false;
        }

        let todo_count = db.todos(&list.id).map(|t| t.len()).unwrap_or(0);
        self.mode = AppMode::TodosHostReassign(TodosHostReassignState {
            project_name: project_name.to_string(),
            deleted_feature_name: deleted_feature_name.to_string(),
            list_id: list.id,
            candidates,
            selected: 0,
            todo_count,
        });
        true
    }

    /// Move the re-home prompt selection by `delta`, clamped over the
    /// candidates plus the trailing "Delete the list" option.
    pub fn todos_host_reassign_move(&mut self, delta: isize) {
        if let AppMode::TodosHostReassign(state) = &mut self.mode {
            let option_count = state.candidates.len() + 1; // +1 for "Delete"
            if option_count == 0 {
                return;
            }
            let cur = state.selected as isize;
            let next = (cur + delta).rem_euclid(option_count as isize);
            state.selected = next as usize;
        }
    }

    /// Apply the re-home prompt choice: re-home the list onto the selected
    /// surviving feature, or delete the list when "Delete" is chosen.
    pub fn confirm_todos_host_reassign(&mut self) -> Result<()> {
        let (list_id, choice) = match &self.mode {
            AppMode::TodosHostReassign(state) => {
                let choice = state.candidates.get(state.selected).cloned();
                (state.list_id.clone(), choice)
            }
            _ => return Ok(()),
        };

        let message = if let Some((feature_name, feature_id)) = choice {
            if let Some(db) = self.db.as_ref() {
                db.set_todo_list_host_feature(&list_id, &feature_id)?;
            }
            format!("TODO list re-homed to '{feature_name}'")
        } else {
            if let Some(db) = self.db.as_ref() {
                db.delete_todo_list(&list_id)?;
            }
            "TODO list deleted".to_string()
        };

        self.mode = AppMode::Normal;
        self.message = Some(message);
        Ok(())
    }

    /// Cancel the re-home prompt without dropping the list: keep the TODOs by
    /// re-homing onto the first surviving feature (the safe default).
    pub fn cancel_todos_host_reassign(&mut self) -> Result<()> {
        if let AppMode::TodosHostReassign(state) = &mut self.mode {
            state.selected = 0; // first candidate = re-home, never "Delete"
        }
        self.confirm_todos_host_reassign()
    }
}
