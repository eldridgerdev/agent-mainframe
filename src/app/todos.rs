//! Native TODOs overlay: open/close, navigation, and editing. Agent-spawn
//! lands in a later epic (see `docs/backlog/feature-todos-plan.md`).
//!
//! Edits mutate the in-memory [`TodoViewState`] and, when a DB is present,
//! persist the change. The in-memory list is the source of truth for the
//! overlay (so it works without a DB, e.g. in tests).

use anyhow::Result;
use uuid::Uuid;

use crate::app::{App, AppMode, Selection, TodoViewState};
use crate::db::todos::{Todo, TodoPriority};

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

    /// Start editing the list's "left off here" carry-over banner.
    pub fn todos_begin_edit_carry_over(&mut self) {
        let initial = match &self.mode {
            AppMode::Todos(state) => state
                .list
                .as_ref()
                .and_then(|l| l.carry_over.clone())
                .unwrap_or_default(),
            _ => return,
        };
        self.todos_begin_edit(crate::app::TodoEditTarget::CarryOver, initial);
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
            TodoEditTarget::CarryOver => {
                self.todos_set_carry_over(text.trim().to_string())?;
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
            (Some(db), Some(list_id)) => {
                db.add_todo(list_id, &title, None, TodoPriority::Med)?
            }
            _ => Todo {
                id: Uuid::new_v4().to_string(),
                list_id: list_id.unwrap_or_default(),
                title,
                body: None,
                priority: TodoPriority::Med,
                done: false,
                sort_order: next_order,
                spawned_session_id: None,
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

    /// Toggle the selected TODO's done flag.
    pub fn todos_toggle_done(&mut self) -> Result<()> {
        self.todos_update_selected(|t| t.done = !t.done)
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

    /// Update the carry-over banner note, persisting it.
    fn todos_set_carry_over(&mut self, note: String) -> Result<()> {
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
        let prompt = Self::todo_spawn_prompt(&todo);

        // Reuse a still-live linked session if the TODO already has one.
        let existing_si = todo.spawned_session_id.as_ref().and_then(|sid| {
            self.store
                .projects
                .get(pi)
                .and_then(|p| p.features.get(fi))
                .and_then(|f| f.sessions.iter().position(|s| &s.id == sid))
        });

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
                let si = match self.create_agent_session_labeled(pi, fi, &label, Some(agent)) {
                    Ok(si) => si,
                    Err(e) => {
                        self.push_toast_error(format!("Failed to launch agent: {e}"));
                        return Ok(());
                    }
                };
                let session_id = self.store.projects[pi].features[fi].sessions[si].id.clone();
                // Record the link before we leave the overlay (still in Todos mode).
                self.todos_record_spawned_session(&todo.id, &session_id)?;
                si
            }
        };

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

    /// Record the spawned session id on the TODO, in memory and (with a DB) on
    /// disk.
    pub(crate) fn todos_record_spawned_session(
        &mut self,
        todo_id: &str,
        session_id: &str,
    ) -> Result<()> {
        let updated = match &mut self.mode {
            AppMode::Todos(state) => state
                .todos
                .iter_mut()
                .find(|t| t.id == todo_id)
                .map(|t| {
                    t.spawned_session_id = Some(session_id.to_string());
                    t.clone()
                }),
            _ => None,
        };
        if let (Some(db), Some(todo)) = (&self.db, &updated) {
            db.update_todo(todo)?;
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
        todos.sort_by(|a, b| {
            a.done
                .cmp(&b.done)
                .then(a.sort_order.cmp(&b.sort_order))
        });
    }
}
