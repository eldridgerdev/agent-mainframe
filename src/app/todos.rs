//! Native TODOs overlay: open/close, navigation, and editing. Agent-spawn
//! lands in a later epic (see `docs/backlog/feature-todos-plan.md`).
//!
//! Edits mutate the in-memory [`TodoViewState`] and, when a DB is present,
//! persist the change. The in-memory list is the source of truth for the
//! overlay (so it works without a DB, e.g. in tests).

use anyhow::Result;
use uuid::Uuid;

use crate::app::{
    App, AppMode, Selection, StartIntent, TodoImplementChoice, TodoImplementChoiceState,
    TodoImplementNextContext, TodoLaunchAction, TodoLaunchStep, TodoPlanDestination,
    TodoPlanOrigin, TodoViewState, TodosHostReassignState,
};
use crate::db::todos::{Todo, TodoPriority};

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
    /// The list-level scratchpad note (`todo_lists.carry_over`).
    pub scratchpad: Option<String>,
}

/// What [`App::next_todo_index`] settled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NextTodo {
    /// Nothing has been started for it yet: eligible for an atomic claim.
    Unstarted(usize),
    /// No unstarted item remains; this linked item is held in reserve and may
    /// only be reported as status.
    Reserved(usize),
    /// No eligible or reserved item remains.
    Unavailable,
}

impl App {
    /// Open the native TODOs overlay for the TODOs session at `(pi, fi, si)`.
    /// Loads the project's list and items from the DB; with no DB (tests) it
    /// opens an empty list.
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

        let (list, todos) = self.load_todos_for_project(&project_id);

        self.mode = AppMode::Todos(TodoViewState {
            project_id,
            pi,
            fi,
            project_name,
            feature_name,
            list,
            todos,
            selected: 0,
            scroll_offset: 0,
            editor: None,
            pending_delete: false,
            launch: None,
        });
        Ok(())
    }

    /// Load `(list, todos)` for a project from the DB, or `(None, empty)` when
    /// no DB is available.
    fn load_todos_for_project(
        &mut self,
        project_id: &str,
    ) -> (
        Option<crate::db::todos::TodoList>,
        Vec<crate::db::todos::Todo>,
    ) {
        let Some(db) = &self.db else {
            return (None, Vec::new());
        };
        let list = match db.todo_list(project_id) {
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

    pub fn todos_select_next(&mut self) {
        if let AppMode::Todos(state) = &mut self.mode {
            let len = state.todos.len();
            if len > 0 {
                state.selected = (state.selected + 1) % len;
            }
        }
    }

    pub fn todos_select_prev(&mut self) {
        if let AppMode::Todos(state) = &mut self.mode {
            let len = state.todos.len();
            if len > 0 {
                state.selected = if state.selected == 0 {
                    len - 1
                } else {
                    state.selected - 1
                };
            }
        }
    }

    // ----- quick-capture from a session view ----------------------------

    /// Open the one-line TODO quick-capture over the current session view. The
    /// typed title is appended to the current project's list on commit. No-op
    /// unless a session view is active.
    pub fn open_todo_quick_capture(&mut self) {
        let view = match &self.mode {
            AppMode::Viewing(view) => view.clone(),
            _ => return,
        };
        let project_name = view.project_name.clone();
        self.mode = AppMode::TodoQuickCapture(crate::app::TodoQuickCaptureState {
            view,
            project_name,
            input: String::new(),
        });
    }

    /// Cancel quick-capture, returning to the session view unchanged.
    pub fn cancel_todo_quick_capture(&mut self) {
        if let AppMode::TodoQuickCapture(state) = &self.mode {
            self.mode = AppMode::Viewing(state.view.clone());
        }
    }

    /// Append the typed title to the current project's TODO list, then return to
    /// the session view. An empty title is a no-op cancel. If the project has no
    /// TODOs session yet, one is created (with its list) under the current
    /// feature before the item is appended.
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

    /// Ensure the project has a TODOs session + list (creating one under feature
    /// `fi` when it doesn't), then append `title` to the list.
    fn quick_capture_todo(&mut self, pi: usize, fi: usize, title: &str) -> Result<()> {
        // No-op create when the project already has a TODOs session; otherwise
        // this adds one (and its list) under the current feature.
        self.add_todos_session_for_picker(pi, fi, None)?;

        let (project_id, feature_id) = match self.store.projects.get(pi) {
            Some(project) => (
                project.id.clone(),
                project.features.get(fi).map(|f| f.id.clone()),
            ),
            None => anyhow::bail!("project not found"),
        };
        let feature_id = feature_id.unwrap_or_default();

        // Persist only when a DB is present; without one (tests) the session was
        // still created in memory, matching the overlay's DB-optional behavior.
        if let Some(db) = &self.db {
            let list = db.load_or_create_todo_list(&project_id, &feature_id)?;
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

    /// Start adding a new TODO (empty title editor).
    pub fn todos_begin_add(&mut self) {
        self.todos_begin_edit(crate::app::TodoEditTarget::New, String::new());
    }

    /// Start editing the selected TODO's title.
    pub fn todos_begin_edit_title(&mut self) {
        let initial = match &self.mode {
            AppMode::Todos(state) => state.todos.get(state.selected).map(|t| t.title.clone()),
            _ => None,
        };
        if let Some(initial) = initial {
            self.todos_begin_edit(crate::app::TodoEditTarget::Title, initial);
        }
    }

    /// Start editing the selected TODO's notes/detail body.
    pub fn todos_begin_edit_notes(&mut self) {
        let initial = match &self.mode {
            AppMode::Todos(state) => state
                .todos
                .get(state.selected)
                .map(|t| t.body.clone().unwrap_or_default()),
            _ => None,
        };
        if let Some(initial) = initial {
            self.todos_begin_edit(crate::app::TodoEditTarget::Notes, initial);
        }
    }

    /// Start editing the list's free-form scratchpad note.
    pub fn todos_begin_edit_scratchpad(&mut self) {
        let initial = match &self.mode {
            AppMode::Todos(state) => state
                .list
                .as_ref()
                .and_then(|l| l.carry_over.clone())
                .unwrap_or_default(),
            _ => return,
        };
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

    /// Resolve the list id for the current overlay, creating the list (hosted
    /// by the current feature) on first write. With a DB this persists the
    /// list; without one it synthesizes an in-memory list so edits still work.
    fn todos_ensure_list_id(&mut self) -> Option<String> {
        if let AppMode::Todos(state) = &self.mode
            && let Some(list) = &state.list
        {
            return Some(list.id.clone());
        }
        let (project_id, pi, fi) = match &self.mode {
            AppMode::Todos(state) => (state.project_id.clone(), state.pi, state.fi),
            _ => return None,
        };
        let feature_id = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.id.clone())
            .unwrap_or_default();

        let list = match self.db.as_ref() {
            Some(db) => match db.load_or_create_todo_list(&project_id, &feature_id) {
                Ok(list) => list,
                Err(e) => {
                    self.log_error("todos", format!("failed to create todo list: {e}"));
                    return None;
                }
            },
            None => crate::db::todos::TodoList {
                id: Uuid::new_v4().to_string(),
                project_id,
                feature_id,
                carry_over: None,
                created_at: String::new(),
                updated_at: String::new(),
            },
        };
        let id = list.id.clone();
        if let AppMode::Todos(state) = &mut self.mode {
            state.list = Some(list);
        }
        Some(id)
    }

    /// Append a new TODO with `title`, persisting and selecting it.
    fn todos_add(&mut self, title: String) -> Result<()> {
        let list_id = self.todos_ensure_list_id();

        // Persist via DB when available; otherwise build an in-memory item.
        let next_order = match &self.mode {
            AppMode::Todos(state) => state
                .todos
                .iter()
                .map(|t| t.sort_order)
                .max()
                .map(|m| m + 1)
                .unwrap_or(0),
            _ => 0,
        };

        let new_todo = match (&self.db, &list_id) {
            (Some(db), Some(list_id)) => db.add_todo(list_id, &title, None, TodoPriority::Med)?,
            _ => Todo {
                id: Uuid::new_v4().to_string(),
                list_id: list_id.unwrap_or_default(),
                title,
                body: None,
                priority: TodoPriority::Med,
                done: false,
                sort_order: next_order,
                spawned_session_id: None,
                linked_feature_id: None,
                in_progress: false,
                created_at: String::new(),
                updated_at: String::new(),
            },
        };

        let new_id = new_todo.id.clone();
        if let AppMode::Todos(state) = &mut self.mode {
            state.todos.push(new_todo);
            Self::resort_todos(&mut state.todos);
            if let Some(pos) = state.todos.iter().position(|t| t.id == new_id) {
                state.selected = pos;
            }
        }
        Ok(())
    }

    /// Mutate the selected TODO in place, persisting the change.
    fn todos_update_selected(&mut self, f: impl FnOnce(&mut Todo)) -> Result<()> {
        let updated = match &mut self.mode {
            AppMode::Todos(state) => match state.todos.get_mut(state.selected) {
                Some(todo) => {
                    f(todo);
                    todo.clone()
                }
                None => return Ok(()),
            },
            _ => return Ok(()),
        };
        if let Some(db) = &self.db {
            db.update_todo(&updated)?;
        }
        // Re-sort in case `done` changed, keeping the cursor on the same item.
        if let AppMode::Todos(state) = &mut self.mode {
            Self::resort_todos(&mut state.todos);
            if let Some(pos) = state.todos.iter().position(|t| t.id == updated.id) {
                state.selected = pos;
            }
        }
        Ok(())
    }

    /// Toggle the selected TODO's done flag. Completing an item ends whatever
    /// was underway on it, so the in-progress flag goes with it.
    pub fn todos_toggle_done(&mut self) -> Result<()> {
        self.todos_update_selected(|t| {
            t.done = !t.done;
            if t.done {
                t.in_progress = false;
            }
        })
    }

    /// Toggle the selected TODO's in-progress flag by hand.
    ///
    /// The flag is set automatically when an agent is launched, but it has to
    /// be clearable: a session the user abandoned without closing would
    /// otherwise keep the item out of "implement next" for good. Marking an
    /// item underway also un-completes it — the two states contradict each
    /// other, and the one just asked for is the one that wins.
    pub fn todos_toggle_in_progress(&mut self) -> Result<()> {
        self.todos_update_selected(|t| {
            t.in_progress = !t.in_progress;
            if t.in_progress {
                t.done = false;
            }
        })
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
    /// display order, persisting the new `sort_order` for the whole list.
    pub fn todos_reorder(&mut self, delta: isize) -> Result<()> {
        let ids: Vec<String> = match &mut self.mode {
            AppMode::Todos(state) => {
                let len = state.todos.len();
                if len < 2 {
                    return Ok(());
                }
                let cur = state.selected;
                let target = cur as isize + delta;
                if target < 0 || target as usize >= len {
                    return Ok(());
                }
                let target = target as usize;
                state.todos.swap(cur, target);
                // Renumber sort_order to the new display positions.
                for (i, todo) in state.todos.iter_mut().enumerate() {
                    todo.sort_order = i as i64;
                }
                Self::resort_todos(&mut state.todos);
                // Follow the moved item.
                let moved_id = state.todos[target].id.clone();
                if let Some(pos) = state.todos.iter().position(|t| t.id == moved_id) {
                    state.selected = pos;
                }
                state.todos.iter().map(|t| t.id.clone()).collect()
            }
            _ => return Ok(()),
        };
        if let Some(db) = &self.db {
            db.reorder_todos(&ids)?;
        }
        Ok(())
    }

    /// Update the list's scratchpad note, persisting it. (Stored in the legacy
    /// `carry_over` column / `set_todo_carry_over` DB method.)
    fn todos_set_scratchpad(&mut self, note: String) -> Result<()> {
        let list_id = self.todos_ensure_list_id();
        let value = if note.is_empty() { None } else { Some(note) };
        if let (Some(db), Some(list_id)) = (&self.db, &list_id) {
            db.set_todo_carry_over(list_id, value.as_deref())?;
        }
        if let AppMode::Todos(state) = &mut self.mode
            && let Some(list) = &mut state.list
        {
            list.carry_over = value;
        }
        Ok(())
    }

    // ----- spawn agent ---------------------------------------------------

    /// Launch (or reuse) an agent session for the selected TODO, then seed the
    /// composer with a generated prompt — editable, not submitted.
    ///
    /// The session is created in the list's host feature (resolved from
    /// `list.feature_id`, falling back to the feature the TODOs session lives
    /// under) using that feature's configured agent and mode/flags. If the TODO
    /// already links a session that is still live, that session is reused (jumped
    /// to and added onto) instead of spawning a second. The launched session's
    /// id is recorded on the TODO so the list can show it as "launched".
    /// `g`/`Enter` on the selected TODO.
    ///
    /// Resolves what the key means before offering a choice, because a TODO
    /// that already has somewhere to go should go there rather than ask again:
    ///
    /// 1. A linked feature (a previous plan-mode run created one) — jump to it.
    /// 2. A live linked session — jump to it, as this key always has.
    /// 3. Otherwise — open the chooser.
    ///
    /// The feature link wins when a TODO carries both. A feature is the larger
    /// destination: the session link points at one agent inside the host
    /// feature, while the feature link points at a whole checkout created for
    /// this item, and that is where the work moved to.
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

        // 2. A still-live session spawned for this TODO.
        if let Some(session_id) = todo.spawned_session_id.as_deref()
            && let Some(si) = self.session_index_in_feature(pi, fi, session_id)
        {
            self.selection = Selection::Session(pi, fi, si);
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
            AppMode::Todos(state) => Some(SelectedTodoContext {
                todo: state.todos.get(state.selected).cloned()?,
                pi: state.pi,
                fallback_fi: state.fi,
                host_feature_id: state.list.as_ref().map(|l| l.feature_id.clone()),
                list_id: state.list.as_ref().map(|l| l.id.clone()),
                scratchpad: state.list.as_ref().and_then(|l| l.carry_over.clone()),
            }),
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

    fn session_index_in_feature(&self, pi: usize, fi: usize, session_id: &str) -> Option<usize> {
        self.store
            .projects
            .get(pi)?
            .features
            .get(fi)?
            .sessions
            .iter()
            .position(|s| s.id == session_id)
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
    /// overlay row, for the same reason [`Self::todos_mark_started`] is: this
    /// also runs from the dashboard's "implement next", where no overlay is
    /// open and there is no in-memory row to write back.
    fn clear_todo_linked_feature(&mut self, todo_id: &str) -> Result<()> {
        if let AppMode::Todos(state) = &mut self.mode
            && let Some(todo) = state.todos.iter_mut().find(|t| t.id == todo_id)
        {
            todo.linked_feature_id = None;
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
                let fi = self.resolve_todo_host_feature(
                    state.pi,
                    state.list.as_ref().map(|l| l.feature_id.as_str()),
                    state.fi,
                );
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

    pub fn todos_spawn_agent(&mut self) -> Result<()> {
        let (todo, host_feature_id, fallback_fi, pi) = match &self.mode {
            AppMode::Todos(state) => {
                let Some(todo) = state.todos.get(state.selected).cloned() else {
                    self.push_toast_warning("No TODO selected");
                    return Ok(());
                };
                let host_feature_id = state.list.as_ref().map(|l| l.feature_id.clone());
                (todo, host_feature_id, state.fi, state.pi)
            }
            _ => return Ok(()),
        };

        let fi = self.resolve_todo_host_feature(pi, host_feature_id.as_deref(), fallback_fi);
        self.spawn_todo_agent(pi, fi, &todo, false)
    }

    /// Launch an agent on `todo` in feature `(pi, fi)` and seed the composer
    /// with it, editable and unsent.
    ///
    /// A TODO that already links a live session reuses it — jumped to and added
    /// onto — rather than accumulating a second agent for the same item, unless
    /// `force_new` says the user asked for exactly that. Either way the TODO is
    /// marked started before the view changes, so the list can show it as
    /// underway and "implement next" scans past it.
    ///
    /// Takes the TODO by value rather than reading `self.mode`, because the
    /// dashboard's "implement next" spawns with no overlay open.
    fn spawn_todo_agent(
        &mut self,
        pi: usize,
        fi: usize,
        todo: &Todo,
        force_new: bool,
    ) -> Result<()> {
        let prompt = Self::todo_spawn_prompt(todo);

        let existing_si = if force_new {
            None
        } else {
            todo.spawned_session_id
                .as_deref()
                .and_then(|sid| self.session_index_in_feature(pi, fi, sid))
        };

        let si = match existing_si {
            Some(si) => si,
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
                    Ok(si) => si,
                    Err(e) => {
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
            self.push_toast_error("The session for this TODO vanished as it was created");
            return Ok(());
        };
        // Record the link and the in-progress flag before we leave the overlay
        // (still in Todos mode, so the in-memory list stays truthful too).
        self.todos_mark_started(&todo.id, &session_id)?;

        // Switch into the session view and seed the composer (editable). The
        // seed is not submitted, so the user reviews it before sending.
        self.selection = Selection::Session(pi, fi, si);
        self.enter_view_without_auto_compose()?;
        self.open_compose_seeded(prompt)?;
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
    /// find is worse than one that says the session is gone.
    pub(crate) fn todo_provenance(&self, pi: usize, fi: usize, todo: &Todo) -> Vec<String> {
        let mut lines = Vec::new();

        if let Some(session_id) = todo.spawned_session_id.as_deref() {
            let found = self
                .store
                .projects
                .get(pi)
                .and_then(|project| project.features.get(fi))
                .and_then(|feature| {
                    feature
                        .sessions
                        .iter()
                        .find(|session| session.id == session_id)
                        .map(|session| (session.label.clone(), feature.clone()))
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

    /// Record that work has started on a TODO: its session link and its
    /// in-progress flag, in memory and (with a DB) on disk.
    ///
    /// The DB writes are targeted rather than a whole-row [`update_todo`]
    /// because this is called from the dashboard's "implement next" as well,
    /// where no overlay is open and there is no in-memory row to write back —
    /// and reconstructing one would overwrite whatever else changed meanwhile.
    pub(crate) fn todos_mark_started(&mut self, todo_id: &str, session_id: &str) -> Result<()> {
        if let AppMode::Todos(state) = &mut self.mode
            && let Some(todo) = state.todos.iter_mut().find(|t| t.id == todo_id)
        {
            todo.spawned_session_id = Some(session_id.to_string());
            todo.in_progress = true;
        }
        if let Some(db) = &self.db {
            db.set_todo_spawned_session(todo_id, session_id)?;
            db.set_todo_in_progress(todo_id, true)?;
        }
        Ok(())
    }

    /// Drop any TODO's link to a feature that has just been deleted.
    ///
    /// Separate from [`Self::handle_todos_host_feature_deleted`], which is
    /// about the *list's* home: this is about individual rows that were planned
    /// into the deleted feature. The TODO survives — the work it describes
    /// outlived the branch — and the next `g` offers the chooser again rather
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
            for todo in state
                .todos
                .iter_mut()
                .filter(|t| t.linked_feature_id.as_deref() == Some(feature_id))
            {
                todo.linked_feature_id = None;
            }
        }
    }

    // ----- implement next ------------------------------------------------

    /// `I` on a TODOs session row on the dashboard: take the highest-priority
    /// unstarted TODO and put an agent on it.
    ///
    /// Inert on anything but a TODOs session row — a project with no list has
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
    /// list already loaded, and deliberately distinct from `g`/`Enter`, which
    /// stay on the item under the cursor.
    pub fn implement_next_todo_in_overlay(&mut self) -> Result<()> {
        let AppMode::Todos(state) = &self.mode else {
            return Ok(());
        };
        let (pi, fi) = (state.pi, state.fi);
        let ctx = self.implement_next_ctx(pi, fi);
        self.implement_next(ctx, Vec::new())
    }

    /// Gather the list to scan: the overlay's in-memory rows when it is open
    /// (they are its source of truth, and may hold edits a DB-less run never
    /// persisted), otherwise the project's list read from the DB.
    fn implement_next_ctx(&mut self, pi: usize, fallback_fi: usize) -> TodoImplementNextContext {
        if let AppMode::Todos(state) = &self.mode {
            return TodoImplementNextContext {
                pi: state.pi,
                fallback_fi: state.fi,
                host_feature_id: state.list.as_ref().map(|l| l.feature_id.clone()),
                list_id: state.list.as_ref().map(|l| l.id.clone()),
                todos: state.todos.clone(),
            };
        }
        let project_id = match self.store.projects.get(pi) {
            Some(project) => project.id.clone(),
            None => {
                return TodoImplementNextContext {
                    pi,
                    fallback_fi,
                    host_feature_id: None,
                    list_id: None,
                    todos: Vec::new(),
                };
            }
        };
        let (list, todos) = self.load_todos_for_project(&project_id);
        TodoImplementNextContext {
            pi,
            fallback_fi,
            host_feature_id: list.as_ref().map(|l| l.feature_id.clone()),
            list_id: list.map(|l| l.id),
            todos,
        }
    }

    /// Run the scan and act on what it finds.
    fn implement_next(
        &mut self,
        mut ctx: TodoImplementNextContext,
        skipped: Vec<String>,
    ) -> Result<()> {
        let fi =
            self.resolve_todo_host_feature(ctx.pi, ctx.host_feature_id.as_deref(), ctx.fallback_fi);
        self.todos_reconcile_dead_sessions(ctx.pi, fi, &mut ctx.todos)?;

        match Self::next_todo_index(&ctx.todos, &skipped) {
            NextTodo::Unavailable => {
                self.push_toast_info(Self::no_next_todo_message(&ctx.todos, &skipped));
                Ok(())
            }
            NextTodo::Unstarted(i) => {
                let todo = ctx.todos[i].clone();
                self.spawn_todo_agent(ctx.pi, fi, &todo, false)
            }
            NextTodo::Reserved(i) => {
                let todo = &ctx.todos[i];
                let (todo_id, todo_title) = (todo.id.clone(), todo.title.clone());
                let origin = std::mem::replace(&mut self.mode, AppMode::Normal);
                self.mode = AppMode::TodoImplementChoice(Box::new(TodoImplementChoiceState {
                    origin: Box::new(origin),
                    pi: ctx.pi,
                    fallback_fi: ctx.fallback_fi,
                    host_feature_id: ctx.host_feature_id,
                    todo_id,
                    todo_title,
                    skipped_ids: skipped,
                    selected: 0,
                }));
                Ok(())
            }
        }
    }

    /// The TODO "implement next" should act on, or `None` if there is none.
    ///
    /// Priority first (High, then Med, then Low), and within a priority the
    /// order the list is already in — the sort is stable, so a manual ordering
    /// the user arranged is what breaks ties. Completed, in-progress, and
    /// explicitly skipped items are passed over entirely.
    ///
    /// A TODO that already links a session or a planned feature is not
    /// *chosen*, it is held in reserve: an unstarted item anywhere in the scan
    /// wins over it, and it is only returned — as [`NextTodo::Reserved`], for
    /// the caller to ask about — when nothing unstarted remains. That is what
    /// reconciles "skip TODOs that already have a session" with there being a
    /// prompt for exactly that case.
    pub(crate) fn next_todo_index(todos: &[Todo], skipped_ids: &[String]) -> NextTodo {
        let mut order: Vec<usize> = (0..todos.len()).collect();
        order.sort_by_key(|&i| todos[i].priority.rank());

        let mut started: Option<usize> = None;
        for i in order {
            let todo = &todos[i];
            if todo.done || todo.in_progress || skipped_ids.iter().any(|id| id == &todo.id) {
                continue;
            }
            if todo.spawned_session_id.is_none() && todo.linked_feature_id.is_none() {
                return NextTodo::Unstarted(i);
            }
            if started.is_none() {
                started = Some(i);
            }
        }
        started
            .map(NextTodo::Reserved)
            .unwrap_or(NextTodo::Unavailable)
    }

    /// Why the scan came back empty, said in the terms the user can act on.
    ///
    /// A blanket "nothing to do" would be wrong in the case that matters: items
    /// are there, they are just all underway, and the fix is to finish or
    /// un-flag one rather than to add more.
    pub(crate) fn no_next_todo_message(todos: &[Todo], skipped_ids: &[String]) -> String {
        let open = todos.iter().filter(|t| !t.done).count();
        if open == 0 {
            "No TODOs left to implement".to_string()
        } else if !skipped_ids.is_empty() {
            "No other TODOs left to implement".to_string()
        } else {
            "All remaining TODOs are already in progress".to_string()
        }
    }

    /// Drop session links whose session is gone, and the in-progress flag they
    /// were the evidence for.
    ///
    /// AMF does not clear `spawned_session_id` when a session is removed — the
    /// link is checked at use time instead — so without this a TODO whose agent
    /// was closed would stay marked underway forever and never be offered
    /// again. The flag is only cleared alongside a dead link: an item the user
    /// marked in progress by hand has no session to lose and is left alone.
    fn todos_reconcile_dead_sessions(
        &mut self,
        pi: usize,
        fi: usize,
        todos: &mut [Todo],
    ) -> Result<()> {
        let dead: Vec<String> = todos
            .iter()
            .filter(|t| {
                t.spawned_session_id
                    .as_deref()
                    .is_some_and(|sid| self.session_index_in_feature(pi, fi, sid).is_none())
            })
            .map(|t| t.id.clone())
            .collect();
        if dead.is_empty() {
            return Ok(());
        }

        for todo in todos.iter_mut().filter(|t| dead.contains(&t.id)) {
            todo.spawned_session_id = None;
            todo.in_progress = false;
        }
        if let AppMode::Todos(state) = &mut self.mode {
            for todo in state.todos.iter_mut().filter(|t| dead.contains(&t.id)) {
                todo.spawned_session_id = None;
                todo.in_progress = false;
            }
        }
        if let Some(db) = &self.db {
            for id in &dead {
                db.clear_todo_spawned_session(id)?;
                db.set_todo_in_progress(id, false)?;
            }
        }
        Ok(())
    }

    /// Re-read a TODO by id from whichever list is authoritative right now.
    /// Used when acting on a prompt, so the list changing underneath it is
    /// noticed rather than acted on stale.
    fn find_todo_by_id(&self, pi: usize, todo_id: &str) -> Option<Todo> {
        if let AppMode::Todos(state) = &self.mode {
            return state.todos.iter().find(|t| t.id == todo_id).cloned();
        }
        let db = self.db.as_ref()?;
        let project_id = self.store.projects.get(pi)?.id.clone();
        let list = db.todo_list(&project_id).ok()??;
        db.todos(&list.id)
            .ok()?
            .into_iter()
            .find(|t| t.id == todo_id)
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
        // in-memory list is its source of truth, so a spawn or a re-scan has to
        // see it rather than the empty Normal mode this prompt was holding.
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
                let Some(todo) = self.find_todo_by_id(state.pi, &state.todo_id) else {
                    self.push_toast_warning("That TODO is no longer in the list");
                    return Ok(());
                };
                let fi = self.resolve_todo_host_feature(
                    state.pi,
                    state.host_feature_id.as_deref(),
                    state.fallback_fi,
                );
                if choice == TodoImplementChoice::SpawnNew {
                    return self.spawn_todo_agent(state.pi, fi, &todo, true);
                }
                self.jump_to_started_todo(state.pi, fi, &todo)
            }
        }
    }

    /// Go to whatever an earlier run created for this TODO.
    ///
    /// The feature link wins over the session link for the same reason
    /// [`Self::todos_launch_selected`] prefers it: a planned feature is a whole
    /// checkout made for this item, while the session link is one agent inside
    /// the host feature.
    ///
    /// A link whose feature is gone is *cleared*, exactly as `g`/`Enter` clears
    /// it, and for a sharper reason here: the link is the only thing holding
    /// the TODO back from [`NextTodo::Unstarted`], so leaving it would make every
    /// future "implement next" offer the same item and every jump fail the same
    /// way. Dropping it lets the next scan pick the TODO up and start it.
    fn jump_to_started_todo(&mut self, pi: usize, fi: usize, todo: &Todo) -> Result<()> {
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
        if let Some(si) = todo
            .spawned_session_id
            .as_deref()
            .and_then(|sid| self.session_index_in_feature(pi, fi, sid))
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
        if let AppMode::Todos(state) = &mut self.mode
            && !state.todos.is_empty()
        {
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
        let removed_id = match &mut self.mode {
            AppMode::Todos(state) => {
                state.pending_delete = false;
                if state.todos.is_empty() {
                    return Ok(());
                }
                let id = state.todos[state.selected].id.clone();
                state.todos.remove(state.selected);
                if state.selected >= state.todos.len() {
                    state.selected = state.todos.len().saturating_sub(1);
                }
                id
            }
            _ => return Ok(()),
        };
        if let Some(db) = &self.db {
            db.delete_todo(&removed_id)?;
        }
        Ok(())
    }

    /// Sort items into display order: open first, then by manual `sort_order`.
    fn resort_todos(todos: &mut [Todo]) {
        todos.sort_by(|a, b| a.done.cmp(&b.done).then(a.sort_order.cmp(&b.sort_order)));
    }

    /// Called after a feature is removed from a surviving project. If the
    /// deleted feature hosted the project's TODO list, decide the list's fate:
    ///
    /// - No features remain → silently delete the now-orphaned list.
    /// - Surviving features exist → open [`AppMode::TodosHostReassign`] so the
    ///   user re-homes the list onto another feature or deletes it.
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
        let list = match db.todo_list(&project_id) {
            Ok(Some(list)) => list,
            _ => return false,
        };
        if list.feature_id != deleted_feature_id {
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
