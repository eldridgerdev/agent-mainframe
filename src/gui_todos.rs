//! GUI-facing TODO operations (`AMF_PLAN.md` Task 8, "Share TODO and
//! planning workflows incrementally").
//!
//! Unlike project/feature state, `db::todos` was already presentation-clean
//! before this task -- plain functions over a `&Connection`, marked
//! `#![allow(dead_code)]` "ahead of its UI consumers" per its own doc
//! comment. This module is a thin GUI-request-shaped wrapper over it, not a
//! parallel implementation, exactly the reuse the plan's "retaining TUI
//! adapters" language asks for: the TUI's `app/todos.rs` and this module
//! both end up calling the identical `db::todos`/`AmfDb` functions.
//!
//! TODO lists live outside `ProjectStore`'s full-replace save path (see
//! `db::todos`'s own module doc), so none of Task 5's version-checked-save
//! coordination applies here -- every operation below is already its own
//! atomic SQL statement, safe under GUI/TUI overlap by construction, not by
//! anything added in this task.
//!
//! This module covers list/create, add, status, reorder, move, copy and
//! delete. `gui_contract` owns TODO agent launch and rollback because those
//! require session orchestration. `load` reconciles associations to deleted
//! session records. `gui_plans` adapts the shared Plan Interview engine.
//! Feature-deletion disposition remains tied to a future GUI delete action.

use serde::{Deserialize, Serialize};

// Re-exported (not just `use`d) rather than left inside the private `db`
// module: these types now appear in this module's own public signatures
// below, so the external GUI crate needs a real path to name them --
// `agent_mainframe::gui_todos::Todo`, not an accident of reachability. `db`
// itself stays private; only these specific types are deliberately exported
// here, the same "widen module by module" reasoning `src/lib.rs`'s doc
// comment already states, applied at the type level rather than the module
// level since `db` holds several unrelated submodules that should not all
// become reachable just to export this one's types.
pub use crate::db::todos::{Todo, TodoList, TodoPriority, TodoScope, TodoStatus};
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};

/// A scope key as sent by the frontend. `Worktree` carries a `FeatureTarget`
/// rather than a raw `(project_id, workdir)` pair because the frontend only
/// ever knows a worktree scope *as* "this feature's TODOs" -- resolving the
/// workdir is `GuiHandle::worktree_scope_for_feature`'s job, done once here
/// rather than trusting a client-supplied path.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TodoScopeRequest {
    Worktree(FeatureTarget),
    Project { project_id: String },
    Global,
}

fn resolve_scope(gui: &GuiHandle, request: &TodoScopeRequest) -> GuiResult<TodoScope> {
    Ok(match request {
        TodoScopeRequest::Worktree(target) => {
            let (project_id, workdir) = gui.worktree_scope_for_feature(target)?;
            TodoScope::Worktree {
                project_id,
                workdir,
            }
        }
        TodoScopeRequest::Project { project_id } => TodoScope::Project {
            project_id: project_id.clone(),
        },
        TodoScopeRequest::Global => TodoScope::Global,
    })
}

/// A scope's list plus its items in one call, matching what a TODO panel
/// actually renders -- there is no reason for the frontend to make two
/// round trips for data that is only ever shown together.
#[derive(Debug, Clone, Serialize)]
pub struct TodoListView {
    pub list: TodoList,
    pub todos: Vec<Todo>,
}

fn view_of(gui: &GuiHandle, list: TodoList) -> GuiResult<TodoListView> {
    let todos = gui.db()?.todos(&list.id).map_err(GuiError::from)?;
    Ok(TodoListView { list, todos })
}

/// The scope's list and items, `None` if that scope has no list yet (an
/// untouched scope leaves no row behind -- see `db::todos`'s module doc).
/// The frontend treats `None` as "empty," not an error.
pub fn load(gui: &GuiHandle, request: &TodoScopeRequest) -> GuiResult<Option<TodoListView>> {
    let scope = resolve_scope(gui, request)?;
    // A separate TUI can delete a session while this GUI stays open. Check
    // the persisted session table before returning TODO associations, keeping
    // the TODO's in-progress status when its old session record is gone.
    gui.db()?
        .clear_missing_todo_agent_sessions()
        .map_err(GuiError::from)?;
    match gui.db()?.todo_list(&scope).map_err(GuiError::from)? {
        Some(list) => view_of(gui, list).map(Some),
        None => Ok(None),
    }
}

/// Add a TODO to `request`'s scope, creating the list (hosted by
/// `host_feature_id`, when given) if this is the first item written there.
/// `host_feature_id` is only used on that first-creation path -- an existing
/// list keeps its current host untouched.
pub fn add(
    gui: &GuiHandle,
    request: &TodoScopeRequest,
    host_feature_id: Option<&str>,
    title: &str,
    body: Option<&str>,
    priority: TodoPriority,
) -> GuiResult<TodoListView> {
    let scope = resolve_scope(gui, request)?;
    let db = gui.db()?;
    let list = db
        .load_or_create_todo_list(&scope, host_feature_id)
        .map_err(GuiError::from)?;
    db.add_todo(&list.id, title, body, priority)
        .map_err(GuiError::from)?;
    view_of(gui, list)
}

/// Set a TODO's status directly (a checkbox or explicit control, unlike the
/// TUI's `i`-key three-state cycle) -- both are equally narrow writes
/// through the same `set_work_state`, so neither is more "real" than the
/// other. The existing `agent_session_id` association is left untouched,
/// matching `TodoWorkState::cycle_manually`'s own reasoning: it stays useful
/// for jumping back to prior work.
pub fn set_status(gui: &GuiHandle, todo_id: &str, status: TodoStatus) -> GuiResult<()> {
    let db = gui.db()?;
    let mut todo = db
        .find_todo_by_id(todo_id)
        .map_err(GuiError::from)?
        .ok_or_else(|| not_found(todo_id))?;
    todo.work.status = status;
    db.set_todo_work_state(todo_id, &todo.work)
        .map_err(GuiError::from)
}

pub fn reorder(gui: &GuiHandle, ordered_ids: &[String]) -> GuiResult<()> {
    gui.db()?.reorder_todos(ordered_ids).map_err(GuiError::from)
}

/// Move a TODO to another scope's list, creating that list (with no host
/// feature) if it does not exist yet. A move keeps `agent_session_id` and
/// `status` -- see `db::todos::move_todo`'s own doc comment for why that
/// (not a copy's reset) is correct here: it is the same work, re-filed.
pub fn move_to(
    gui: &GuiHandle,
    todo_id: &str,
    target: &TodoScopeRequest,
) -> GuiResult<TodoListView> {
    let target_scope = resolve_scope(gui, target)?;
    let db = gui.db()?;
    let target_list = db
        .load_or_create_todo_list(&target_scope, None)
        .map_err(GuiError::from)?;
    db.move_todo(todo_id, &target_list.id)
        .map_err(GuiError::from)?;
    view_of(gui, target_list)
}

/// Copy a TODO to another scope's list, creating that list (with no host
/// feature) if it does not exist yet. Unlike `move_to`, the copy resets
/// `agent_session_id` and a non-completed `status` to not-started -- see
/// `db::todos::copy_todo`'s own doc comment: two panes must never both claim
/// one session for the same work.
pub fn copy_to(
    gui: &GuiHandle,
    todo_id: &str,
    target: &TodoScopeRequest,
) -> GuiResult<TodoListView> {
    let target_scope = resolve_scope(gui, target)?;
    let db = gui.db()?;
    let target_list = db
        .load_or_create_todo_list(&target_scope, None)
        .map_err(GuiError::from)?;
    db.copy_todo(todo_id, &target_list.id)
        .map_err(GuiError::from)?;
    view_of(gui, target_list)
}

pub fn delete(gui: &GuiHandle, todo_id: &str) -> GuiResult<()> {
    gui.db()?.delete_todo(todo_id).map_err(GuiError::from)
}

fn not_found(todo_id: &str) -> GuiError {
    GuiError {
        kind: crate::gui_contract::GuiErrorKind::NotFound,
        message: format!("TODO '{todo_id}' was not found (it may have been deleted elsewhere)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::project::{AgentKind, Feature, Project, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::path::PathBuf;

    /// TODO lists live outside `ProjectStore` (see `db::todos`'s module
    /// doc), so these tests only need a real `AmfDb` -- no tmux interaction
    /// happens anywhere in this module, so an empty `MockTmuxOps` (no
    /// expectations at all) is enough to build an `App`.
    fn fixture() -> (GuiHandle, FeatureTarget, tempfile::NamedTempFile) {
        let mut feature = Feature::new_for_project(
            "demo",
            "my-feat".to_string(),
            "my-feat".to_string(),
            PathBuf::from("/tmp/gui-todos-test-workdir"),
            false,
            VibeMode::default(),
            false,
            false,
            AgentKind::default(),
            false,
            false,
        );
        feature.id = "feat-1".to_string();
        let feature_id = feature.id.clone();

        let mut project = Project::new(
            "demo".to_string(),
            PathBuf::from("/tmp/gui-todos-test-repo"),
            false,
            AgentKind::default(),
        );
        project.id = "proj-1".to_string();
        let project_id = project.id.clone();
        project.features.push(feature);

        let mut store = ProjectStore::empty();
        store.projects.push(project);

        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = crate::db::AmfDb::open(tmp_db.path()).unwrap();
        db.save_store(&store).unwrap();

        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.db = Some(db);
        app.store_version = None;

        (
            GuiHandle::from_app(app),
            FeatureTarget {
                project_id,
                feature_id,
            },
            tmp_db,
        )
    }

    #[test]
    fn an_untouched_scope_loads_as_none() {
        let (gui, target, _tmp) = fixture();

        let loaded = load(&gui, &TodoScopeRequest::Worktree(target)).unwrap();

        assert!(loaded.is_none());
    }

    #[test]
    fn add_creates_the_list_on_first_write_and_is_then_visible() {
        let (gui, target, _tmp) = fixture();
        let request = TodoScopeRequest::Worktree(target.clone());

        let view = add(
            &gui,
            &request,
            Some(&target.feature_id),
            "write the plan",
            None,
            TodoPriority::High,
        )
        .unwrap();
        assert_eq!(view.todos.len(), 1);
        assert_eq!(view.todos[0].title, "write the plan");
        assert_eq!(
            view.list.feature_id.as_deref(),
            Some(target.feature_id.as_str())
        );

        let reloaded = load(&gui, &request).unwrap().expect("list now exists");
        assert_eq!(reloaded.todos.len(), 1);
    }

    #[test]
    fn set_status_updates_without_disturbing_other_fields() {
        let (gui, target, _tmp) = fixture();
        let request = TodoScopeRequest::Worktree(target.clone());
        let view = add(&gui, &request, None, "item", None, TodoPriority::Med).unwrap();
        let todo_id = view.todos[0].id.clone();

        set_status(&gui, &todo_id, TodoStatus::Completed).unwrap();

        let reloaded = load(&gui, &request).unwrap().unwrap();
        assert_eq!(reloaded.todos[0].work.status, TodoStatus::Completed);
        assert_eq!(reloaded.todos[0].title, "item");
    }

    #[test]
    fn set_status_reports_not_found_for_an_unknown_todo() {
        let (gui, _target, _tmp) = fixture();

        let err = set_status(&gui, "does-not-exist", TodoStatus::Completed).unwrap_err();

        assert_eq!(err.kind, crate::gui_contract::GuiErrorKind::NotFound);
    }

    #[test]
    fn move_preserves_status_and_session_copy_resets_them() {
        let (gui, target, _tmp) = fixture();
        let worktree = TodoScopeRequest::Worktree(target.clone());
        let global = TodoScopeRequest::Global;

        let view = add(
            &gui,
            &worktree,
            None,
            "shared work",
            None,
            TodoPriority::Med,
        )
        .unwrap();
        let todo_id = view.todos[0].id.clone();
        set_status(&gui, &todo_id, TodoStatus::InProgress).unwrap();
        gui.db()
            .unwrap()
            .set_todo_agent_session(&todo_id, "session-abc")
            .unwrap();

        let moved = move_to(&gui, &todo_id, &global).unwrap();
        assert_eq!(moved.todos[0].work.status, TodoStatus::InProgress);
        assert_eq!(
            moved.todos[0].work.agent_session_id.as_deref(),
            Some("session-abc")
        );
        // The worktree list is now empty -- this was a move, not a copy.
        assert!(load(&gui, &worktree).unwrap().unwrap().todos.is_empty());

        let project_scope = TodoScopeRequest::Project {
            project_id: target.project_id.clone(),
        };
        let copied = copy_to(&gui, &moved.todos[0].id, &project_scope).unwrap();
        assert_eq!(copied.todos[0].work.status, TodoStatus::NotStarted);
        assert_eq!(copied.todos[0].work.agent_session_id, None);
        // The copy leaves the source (global) list's item exactly as it was.
        let global_after_copy = load(&gui, &global).unwrap().unwrap();
        assert_eq!(
            global_after_copy.todos[0].work.status,
            TodoStatus::InProgress
        );
    }

    #[test]
    fn reorder_and_delete_round_trip() {
        let (gui, target, _tmp) = fixture();
        let request = TodoScopeRequest::Worktree(target);
        let a = add(&gui, &request, None, "a", None, TodoPriority::Med)
            .unwrap()
            .todos[0]
            .id
            .clone();
        let view_b = add(&gui, &request, None, "b", None, TodoPriority::Med).unwrap();
        let b = view_b
            .todos
            .iter()
            .find(|t| t.title == "b")
            .unwrap()
            .id
            .clone();

        reorder(&gui, &[b.clone(), a.clone()]).unwrap();
        let reloaded = load(&gui, &request).unwrap().unwrap();
        assert_eq!(reloaded.todos[0].id, b);
        assert_eq!(reloaded.todos[1].id, a);

        delete(&gui, &a).unwrap();
        let after_delete = load(&gui, &request).unwrap().unwrap();
        assert_eq!(after_delete.todos.len(), 1);
        assert_eq!(after_delete.todos[0].id, b);
    }
}
