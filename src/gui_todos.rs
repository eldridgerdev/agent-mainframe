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
pub use crate::db::todos::{Todo, TodoFields, TodoList, TodoPriority, TodoScope, TodoStatus};
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

/// An edit of a TODO's title, notes and priority. `base` is what the editor
/// opened with; the write only lands if the row still holds it (and still sits
/// in `list_id`), so a stale editor can never silently overwrite a newer edit.
#[derive(Debug, Clone, Deserialize)]
pub struct TodoEditRequest {
    pub list_id: String,
    pub todo_id: String,
    pub base: TodoFields,
    pub fields: TodoFields,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TodoEditOutcome {
    Saved {
        todo: Todo,
    },
    /// The TODO changed (or moved lists) since the editor opened. `current`
    /// is what is stored now; nothing was written.
    Conflict {
        current: Todo,
    },
    Gone,
}

const MAX_TITLE_CHARS: usize = 300;

fn normalize_fields(fields: &TodoFields) -> GuiResult<TodoFields> {
    let title = fields.title.trim().to_string();
    if title.is_empty() {
        return Err(invalid("A TODO needs a title"));
    }
    if title.contains(['\n', '\r']) {
        return Err(invalid("A TODO title is one line; put detail in the notes"));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(invalid(format!(
            "A TODO title is limited to {MAX_TITLE_CHARS} characters; put detail in the notes"
        )));
    }
    let body = fields
        .body
        .as_deref()
        .map(str::trim_end)
        .filter(|b| !b.trim().is_empty())
        .map(str::to_string);
    Ok(TodoFields {
        title,
        body,
        priority: fields.priority,
    })
}

/// Edit title, notes and priority. Status, agent association and links are
/// never written here (only `TodoWorkState` transitions change those).
pub fn update(gui: &GuiHandle, request: &TodoEditRequest) -> GuiResult<TodoEditOutcome> {
    let new = normalize_fields(&request.fields)?;
    let db = gui.db()?;
    let wrote = db
        .update_todo_fields_if(&request.todo_id, &request.list_id, &request.base, &new)
        .map_err(GuiError::from)?;
    let current = db
        .find_todo_by_id(&request.todo_id)
        .map_err(GuiError::from)?;
    Ok(match (wrote, current) {
        (true, Some(todo)) => TodoEditOutcome::Saved { todo },
        (false, Some(current)) => TodoEditOutcome::Conflict { current },
        (_, None) => TodoEditOutcome::Gone,
    })
}

/// A scratchpad edit. `list_id` is `None` when the editor opened on a scope
/// with no list yet; `base` is the scratchpad text it opened with.
#[derive(Debug, Clone, Deserialize)]
pub struct ScratchpadEditRequest {
    pub scope: TodoScopeRequest,
    pub host_feature_id: Option<String>,
    pub list_id: Option<String>,
    pub base: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ScratchpadOutcome {
    Saved {
        list_id: Option<String>,
        carry_over: Option<String>,
    },
    Conflict {
        list_id: Option<String>,
        current: Option<String>,
    },
    Gone,
}

fn normalize_scratchpad(text: Option<&str>) -> Option<String> {
    text.map(str::trim_end)
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
}

pub fn update_scratchpad(
    gui: &GuiHandle,
    request: &ScratchpadEditRequest,
) -> GuiResult<ScratchpadOutcome> {
    let scope = resolve_scope(gui, &request.scope)?;
    let db = gui.db()?;
    let new = normalize_scratchpad(request.text.as_deref());
    let base = normalize_scratchpad(request.base.as_deref());
    let current = db.todo_list(&scope).map_err(GuiError::from)?;
    let list = match (current, request.list_id.as_deref()) {
        (Some(list), Some(id)) if list.id == id => list,
        (Some(list), None) => {
            return Ok(ScratchpadOutcome::Conflict {
                list_id: Some(list.id),
                current: list.carry_over,
            });
        }
        (Some(list), Some(_)) => {
            return Ok(ScratchpadOutcome::Conflict {
                list_id: Some(list.id),
                current: list.carry_over,
            });
        }
        (None, Some(_)) => return Ok(ScratchpadOutcome::Gone),
        // An untouched scope has no row; clearing nothing writes nothing.
        (None, None) if new.is_none() => {
            return Ok(ScratchpadOutcome::Saved {
                list_id: None,
                carry_over: None,
            });
        }
        (None, None) => db
            .load_or_create_todo_list(&scope, request.host_feature_id.as_deref())
            .map_err(GuiError::from)?,
    };
    if db
        .set_todo_carry_over_if(&list.id, base.as_deref(), new.as_deref())
        .map_err(GuiError::from)?
    {
        return Ok(ScratchpadOutcome::Saved {
            list_id: Some(list.id),
            carry_over: new,
        });
    }
    let latest = db.todo_list_by_id(&list.id).map_err(GuiError::from)?;
    Ok(match latest {
        Some(list) => ScratchpadOutcome::Conflict {
            list_id: Some(list.id),
            current: list.carry_over,
        },
        None => ScratchpadOutcome::Gone,
    })
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

fn invalid(message: impl Into<String>) -> GuiError {
    GuiError {
        kind: crate::gui_contract::GuiErrorKind::Internal,
        message: message.into(),
    }
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

    fn fields(title: &str, body: Option<&str>, priority: TodoPriority) -> TodoFields {
        TodoFields {
            title: title.to_string(),
            body: body.map(str::to_string),
            priority,
        }
    }

    fn seeded(gui: &GuiHandle, target: &FeatureTarget) -> Todo {
        add(
            gui,
            &TodoScopeRequest::Worktree(target.clone()),
            Some(&target.feature_id),
            "original",
            Some("notes"),
            TodoPriority::Med,
        )
        .unwrap()
        .todos
        .remove(0)
    }

    #[test]
    fn edit_saves_text_and_priority_but_never_the_work_state() {
        let (gui, target, _tmp) = fixture();
        let todo = seeded(&gui, &target);
        // A launch claim lands between opening the editor and saving.
        assert!(
            gui.db()
                .unwrap()
                .reserve_todo_agent_launch(&todo.id)
                .unwrap()
        );

        let outcome = update(
            &gui,
            &TodoEditRequest {
                list_id: todo.list_id.clone(),
                todo_id: todo.id.clone(),
                base: TodoFields::of(&todo),
                fields: fields("  renamed  ", Some("new notes\n"), TodoPriority::High),
            },
        )
        .unwrap();
        let TodoEditOutcome::Saved { todo: saved } = outcome else {
            panic!("expected saved");
        };
        assert_eq!(saved.title, "renamed");
        assert_eq!(saved.body.as_deref(), Some("new notes"));
        assert_eq!(saved.priority, TodoPriority::High);
        assert_eq!(saved.work.status, TodoStatus::InProgress);
    }

    #[test]
    fn a_stale_edit_is_refused_with_the_current_value() {
        let (gui, target, _tmp) = fixture();
        let todo = seeded(&gui, &target);
        let request = |title: &str| TodoEditRequest {
            list_id: todo.list_id.clone(),
            todo_id: todo.id.clone(),
            base: TodoFields::of(&todo),
            fields: fields(title, None, TodoPriority::Low),
        };
        assert!(matches!(
            update(&gui, &request("first")).unwrap(),
            TodoEditOutcome::Saved { .. }
        ));

        // Same base again: the second editor lost the race.
        let TodoEditOutcome::Conflict { current } = update(&gui, &request("second")).unwrap()
        else {
            panic!("expected conflict");
        };
        assert_eq!(current.title, "first");
        assert_eq!(
            load(&gui, &TodoScopeRequest::Worktree(target))
                .unwrap()
                .unwrap()
                .todos[0]
                .title,
            "first"
        );
    }

    #[test]
    fn an_edit_after_a_move_conflicts_and_after_delete_reports_gone() {
        let (gui, target, _tmp) = fixture();
        let todo = seeded(&gui, &target);
        let moved = move_to(&gui, &todo.id, &TodoScopeRequest::Global).unwrap();
        let req = TodoEditRequest {
            list_id: todo.list_id.clone(),
            todo_id: todo.id.clone(),
            base: TodoFields::of(&todo),
            fields: fields("late", None, TodoPriority::Med),
        };
        let TodoEditOutcome::Conflict { current } = update(&gui, &req).unwrap() else {
            panic!("expected conflict");
        };
        assert_eq!(current.list_id, moved.list.id);
        assert_eq!(current.title, "original");

        delete(&gui, &todo.id).unwrap();
        assert!(matches!(update(&gui, &req).unwrap(), TodoEditOutcome::Gone));
    }

    #[test]
    fn edit_rejects_blank_and_multiline_titles() {
        let (gui, target, _tmp) = fixture();
        let todo = seeded(&gui, &target);
        for bad in ["   ", "a\nb"] {
            let err = update(
                &gui,
                &TodoEditRequest {
                    list_id: todo.list_id.clone(),
                    todo_id: todo.id.clone(),
                    base: TodoFields::of(&todo),
                    fields: fields(bad, None, TodoPriority::Med),
                },
            )
            .unwrap_err();
            assert!(err.message.contains("title"), "{}", err.message);
        }
    }

    #[test]
    fn scratchpad_creates_the_list_lazily_and_refuses_stale_writes() {
        let (gui, target, _tmp) = fixture();
        let scope = TodoScopeRequest::Worktree(target.clone());
        let req = |list_id: Option<String>, base: Option<&str>, text: Option<&str>| {
            ScratchpadEditRequest {
                scope: scope.clone(),
                host_feature_id: Some(target.feature_id.clone()),
                list_id,
                base: base.map(str::to_string),
                text: text.map(str::to_string),
            }
        };
        // Clearing an untouched scope leaves no row behind.
        assert!(matches!(
            update_scratchpad(&gui, &req(None, None, Some("  "))).unwrap(),
            ScratchpadOutcome::Saved { list_id: None, .. }
        ));
        assert!(load(&gui, &scope).unwrap().is_none());

        let ScratchpadOutcome::Saved {
            list_id,
            carry_over,
        } = update_scratchpad(&gui, &req(None, None, Some("left off at X"))).unwrap()
        else {
            panic!("expected saved");
        };
        assert_eq!(carry_over.as_deref(), Some("left off at X"));
        let list_id = list_id.unwrap();

        // An editor opened before that write still thinks the pad is empty.
        let ScratchpadOutcome::Conflict { current, .. } =
            update_scratchpad(&gui, &req(Some(list_id.clone()), None, Some("mine"))).unwrap()
        else {
            panic!("expected conflict");
        };
        assert_eq!(current.as_deref(), Some("left off at X"));

        assert!(matches!(
            update_scratchpad(&gui, &req(Some(list_id), Some("left off at X"), None)).unwrap(),
            ScratchpadOutcome::Saved {
                carry_over: None,
                ..
            }
        ));
        assert_eq!(load(&gui, &scope).unwrap().unwrap().list.carry_over, None);
    }

    #[test]
    fn jump_prefers_the_linked_feature_then_a_known_session_and_heals_dead_links() {
        let (mut gui, target, _tmp) = fixture();
        let todo = seeded(&gui, &target);
        let db_todo = todo.id.clone();
        assert!(matches!(
            gui.todo_work_target(&db_todo).unwrap(),
            crate::gui_contract::TodoWorkTarget::None { .. }
        ));

        // Link to a feature that no longer exists: cleared, then reported.
        gui.db()
            .unwrap()
            .set_todo_linked_feature(&db_todo, "gone")
            .unwrap();
        let crate::gui_contract::TodoWorkTarget::None { message } =
            gui.todo_work_target(&db_todo).unwrap()
        else {
            panic!("expected none");
        };
        assert!(message.contains("link was cleared"));
        assert!(
            gui.db()
                .unwrap()
                .find_todo_by_id(&db_todo)
                .unwrap()
                .unwrap()
                .linked_feature_id
                .is_none()
        );

        // A live linked feature without agent sessions opens the feature.
        gui.db()
            .unwrap()
            .set_todo_linked_feature(&db_todo, &target.feature_id)
            .unwrap();
        let crate::gui_contract::TodoWorkTarget::Feature { target: found } =
            gui.todo_work_target(&db_todo).unwrap()
        else {
            panic!("expected feature");
        };
        assert_eq!(found.feature_id, target.feature_id);
    }
}
