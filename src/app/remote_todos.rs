//! TODOs over Remote Control: the three lists a feature can see (its
//! worktree's, its project's, the global one), with add / status / delete.
//!
//! Writes go straight to the database, so they are refused while the desk
//! has the TODOs overlay open: its in-memory panes are that screen's source
//! of truth (see `AppMode::Todos`) and would neither see a phone's edit nor
//! keep it.

use serde_json::{Value, json};

use crate::db::todos::{TodoPriority, TodoScope, TodoStatus};

use super::resource_gate::StartIntent;
use super::{App, AppMode};

impl App {
    fn remote_todo_db(&self) -> Result<&crate::db::AmfDb, String> {
        self.db
            .as_ref()
            .ok_or_else(|| "TODOs need AMF's database, which isn't available.".to_string())
    }

    fn refuse_while_desk_edits_todos(&self) -> Result<(), String> {
        if matches!(self.mode, AppMode::Todos(_)) {
            return Err("TODOs are open on the desk — close them there to edit from here.".into());
        }
        Ok(())
    }

    /// The scopes a feature's TODO overlay shows, narrowest first.
    fn remote_todo_scopes(&self, pi: usize, fi: usize) -> Vec<TodoScope> {
        let mut scopes = Vec::new();
        scopes.extend(self.worktree_todo_scope(pi, fi));
        scopes.push(TodoScope::Project {
            project_id: self.store.projects[pi].id.clone(),
        });
        scopes.push(TodoScope::Global);
        scopes
    }

    pub(super) fn remote_list_todos(&mut self, feature_id: &str) -> Result<Value, String> {
        let (pi, fi) = self
            .feature_indices_by_id(feature_id)
            .ok_or("That feature no longer exists.")?;
        let db = self.remote_todo_db()?;
        let mut lists = Vec::new();
        for scope in self.remote_todo_scopes(pi, fi) {
            let list = db.todo_list(&scope).map_err(|e| e.to_string())?;
            let items = match &list {
                Some(list) => db.todos(&list.id).map_err(|e| e.to_string())?,
                None => Vec::new(),
            };
            lists.push(json!({
                "scope": scope.as_db_str(),
                "label": self.todo_scope_label(&scope),
                "scratchpad": list.as_ref().and_then(|l| l.carry_over.clone()),
                "items": items.iter().map(|todo| json!({
                    "id": todo.id,
                    "title": todo.title,
                    "body": todo.body,
                    "status": todo.work.status,
                    "priority": todo.priority.as_db_str(),
                })).collect::<Vec<_>>(),
            }));
        }
        Ok(json!({ "lists": lists, "editable": !matches!(self.mode, AppMode::Todos(_)) }))
    }

    pub(super) fn remote_add_todo(
        &mut self,
        feature_id: &str,
        scope: &str,
        title: &str,
    ) -> Result<Value, String> {
        self.refuse_while_desk_edits_todos()?;
        let title = title.trim();
        if title.is_empty() {
            return Err("A TODO needs a title.".into());
        }
        let (pi, fi) = self
            .feature_indices_by_id(feature_id)
            .ok_or("That feature no longer exists.")?;
        self.remote_todo_db()?;
        let scope = match scope {
            "worktree" => self
                .worktree_todo_scope(pi, fi)
                .ok_or("This feature sits on the repo root, so it has no worktree list.")?,
            "project" => TodoScope::Project {
                project_id: self.store.projects[pi].id.clone(),
            },
            "global" => TodoScope::Global,
            other => return Err(format!("Unknown TODO list '{other}'.")),
        };
        // Quick-capture's rule: a list is only reachable at the desk through
        // a TODOs session, so make sure this feature has one (no-op if it
        // already does) before the item exists.
        self.add_todos_session_for_picker(pi, fi, None)
            .map_err(|e| e.to_string())?;
        let host = match scope {
            TodoScope::Global => None,
            _ => Some(self.store.projects[pi].features[fi].id.clone()),
        };
        let db = self.remote_todo_db()?;
        let list = db
            .load_or_create_todo_list(&scope, host.as_deref())
            .map_err(|e| e.to_string())?;
        let todo = db
            .add_todo(&list.id, title, None, TodoPriority::Med)
            .map_err(|e| e.to_string())?;
        Ok(json!(todo.id))
    }

    pub(super) fn remote_set_todo_status(
        &mut self,
        todo_id: &str,
        status: &str,
    ) -> Result<Value, String> {
        self.refuse_while_desk_edits_todos()?;
        let status: TodoStatus = serde_json::from_value(json!(status))
            .map_err(|_| format!("Unknown TODO status '{status}'."))?;
        let db = self.remote_todo_db()?;
        let mut todo = db
            .find_todo_by_id(todo_id)
            .map_err(|e| e.to_string())?
            .ok_or("That TODO no longer exists.")?;
        // Status only; the session link survives, as it does for the desk's
        // manual `i` cycle.
        if status == TodoStatus::Completed {
            todo.work.complete();
        } else {
            todo.work.status = status;
        }
        db.set_todo_work_state(todo_id, &todo.work)
            .map_err(|e| e.to_string())?;
        Ok(Value::Null)
    }

    /// `spawn_todo_agent` without its last step: that one switches the desk
    /// into the new session and seeds its composer, which would take the
    /// screen from whoever is at the desk. The phone gets the prompt back
    /// instead and seeds its own input box.
    pub(super) fn remote_start_todo(
        &mut self,
        feature_id: &str,
        todo_id: &str,
    ) -> Result<Value, String> {
        self.refuse_while_desk_edits_todos()?;
        let (pi, fi) = self
            .feature_indices_by_id(feature_id)
            .ok_or("That feature no longer exists.")?;
        let todo = self
            .remote_todo_db()?
            .find_todo_by_id(todo_id)
            .map_err(|e| e.to_string())?
            .ok_or("That TODO no longer exists.")?;
        if todo.work.status == TodoStatus::Completed {
            return Err("That TODO is done — mark it not started to work on it again.".into());
        }
        let prompt = Self::todo_spawn_prompt(&todo);

        // Work already underway: go to it rather than start a second agent.
        if let Some(session_id) = todo.work.agent_session_id.as_deref()
            && self.session_indices_by_id(session_id).is_some()
        {
            return Ok(json!({ "session_id": session_id, "prompt": null }));
        }
        let reserved_here = todo.work.status != TodoStatus::InProgress;
        if reserved_here
            && !self
                .todos_reserve_launch(&todo)
                .map_err(|e| e.to_string())?
        {
            return Err("That TODO was just started somewhere else.".into());
        }
        let agent = self.store.projects[pi].features[fi].agent.clone();
        let label = Self::todo_session_label(&todo.title);
        let si = match self.create_agent_session_labeled(
            pi,
            fi,
            &label,
            Some(agent),
            StartIntent::Warn("the agent for a TODO from your phone"),
        ) {
            Ok(si) => si,
            Err(e) => {
                if reserved_here {
                    self.todos_rollback_launch_best_effort(&todo.id);
                }
                return Err(format!("Couldn't start the agent: {e}"));
            }
        };
        let session = &mut self.store.projects[pi].features[fi].sessions[si];
        session.todo_reference = Some(crate::project::TodoSessionReference {
            todo_id: todo.id.clone(),
            launched_from_todo_menu: true,
        });
        let session_id = session.id.clone();
        if let Err(e) = self.save() {
            self.log_warn(
                "todos",
                format!("started TODO agent but couldn't save its TODO reference: {e}"),
            );
        }
        self.refresh_active_todos_sidebar_cache();
        if let Err(e) = self.todos_mark_in_progress(&todo.id, Some(&session_id)) {
            if reserved_here {
                self.todos_rollback_launch_best_effort(&todo.id);
            }
            return Err(e.to_string());
        }
        self.push_toast_info(format!(
            "Started an agent on '{}' from your phone",
            todo.title
        ));
        Ok(json!({ "session_id": session_id, "prompt": prompt }))
    }

    pub(super) fn remote_delete_todo(&mut self, todo_id: &str) -> Result<Value, String> {
        self.refuse_while_desk_edits_todos()?;
        let db = self.remote_todo_db()?;
        db.find_todo_by_id(todo_id)
            .map_err(|e| e.to_string())?
            .ok_or("That TODO no longer exists.")?;
        db.delete_todo(todo_id).map_err(|e| e.to_string())?;
        Ok(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use crate::app::remote_server::tests::test_app_with_feature_and_db;
    use crate::remote_server::RemoteAction;

    #[test]
    fn add_list_complete_and_delete_round_trip() {
        let (_db, mut app) = test_app_with_feature_and_db();
        let feature_id = app.store.projects[0].features[0].id.clone();

        let todo_id = app
            .apply_remote_action(RemoteAction::AddTodo {
                feature_id: feature_id.clone(),
                scope: "project".into(),
                title: "  Write the docs  ".into(),
            })
            .unwrap();
        let todo_id = todo_id.as_str().unwrap().to_string();

        let listed = app
            .apply_remote_action(RemoteAction::ListTodos {
                feature_id: feature_id.clone(),
            })
            .unwrap();
        let project = listed["lists"]
            .as_array()
            .unwrap()
            .iter()
            .find(|list| list["scope"] == "project")
            .unwrap();
        assert_eq!(project["items"][0]["title"], "Write the docs");
        assert_eq!(project["items"][0]["status"], "not_started");

        app.apply_remote_action(RemoteAction::SetTodoStatus {
            todo_id: todo_id.clone(),
            status: "completed".into(),
        })
        .unwrap();
        let todo = app.db.as_ref().unwrap().find_todo_by_id(&todo_id).unwrap();
        assert!(todo.unwrap().work.status.is_completed());

        app.apply_remote_action(RemoteAction::DeleteTodo {
            todo_id: todo_id.clone(),
        })
        .unwrap();
        assert!(
            app.db
                .as_ref()
                .unwrap()
                .find_todo_by_id(&todo_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn bad_input_is_refused_with_a_reason() {
        let (_db, mut app) = test_app_with_feature_and_db();
        let feature_id = app.store.projects[0].features[0].id.clone();
        assert_eq!(
            app.apply_remote_action(RemoteAction::AddTodo {
                feature_id: feature_id.clone(),
                scope: "project".into(),
                title: "   ".into(),
            }),
            Err("A TODO needs a title.".into())
        );
        assert_eq!(
            app.apply_remote_action(RemoteAction::SetTodoStatus {
                todo_id: "x".into(),
                status: "done-ish".into(),
            }),
            Err("Unknown TODO status 'done-ish'.".into())
        );
        assert_eq!(
            app.apply_remote_action(RemoteAction::ListTodos {
                feature_id: "nope".into()
            }),
            Err("That feature no longer exists.".into())
        );
    }
}
